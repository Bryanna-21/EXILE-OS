/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "broker.h"

#include <algorithm>
#include <sstream>

#include "sha256.h"

namespace exile::policy {

char const* toString(Outcome o)
{
    switch (o) {
    case Outcome::Allow: return "allow";
    case Outcome::Deny: return "deny";
    case Outcome::NeedsApproval: return "needs-approval";
    }
    return "?";
}

char const* toString(Reason r)
{
    switch (r) {
    case Reason::Granted: return "granted";
    case Reason::AutoLowRisk: return "auto-low-risk";
    case Reason::UnknownPrincipal: return "unknown-principal";
    case Reason::Suspended: return "suspended";
    case Reason::InvalidRequest: return "invalid-request";
    case Reason::NotDeclared: return "not-declared";
    case Reason::ApprovalRequired: return "approval-required";
    case Reason::TooManyPending: return "too-many-pending";
    case Reason::InternalError: return "internal-error";
    }
    return "?";
}

namespace {
constexpr char const* kGenesis = "0000000000000000000000000000000000000000000000000000000000000000";
constexpr size_t kMaxPendingPerPrincipal = 32;
constexpr int64_t kAgentMaxUntilSeconds = 24 * 3600;

std::string esc(std::string_view s)
{
    static char const* hex = "0123456789ABCDEF";
    std::string o;
    for (unsigned char c : s) {
        if (c == '%' || c == '|' || c < 0x20 || c == 0x7f) {
            o += '%';
            o += hex[c >> 4];
            o += hex[c & 15];
        } else {
            o += static_cast<char>(c);
        }
    }
    return o;
}

std::string line(AuditEntry const& e)
{
    return std::to_string(e.seq) + "|" + std::to_string(e.time) + "|" + esc(e.kind) + "|" + esc(e.principal) + "|" + esc(e.permission) + "|"
        + esc(e.outcome) + "|" + esc(e.detail);
}

std::string chain(std::string const& prev, std::string const& l) { return exile::sha256Hex(prev + "\n" + l); }
} // namespace

Broker::Broker(Clock clock)
    : clock_(std::move(clock))
{
}

void Broker::log(std::string kind, std::string principal, std::string perm, std::string outcome, std::string detail)
{
    AuditEntry e;
    e.seq = audit_.size() + 1;
    e.time = clock_();
    e.kind = std::move(kind);
    e.principal = std::move(principal).substr(0, 128);
    e.permission = std::move(perm).substr(0, 600);
    e.outcome = std::move(outcome);
    e.detail = detail.substr(0, 300);
    e.prevHash = audit_.empty() ? kGenesis : audit_.back().hash;
    e.hash = chain(e.prevHash, line(e));
    audit_.push_back(std::move(e));
}

std::string Broker::resolveRoot(std::string const& principal, std::string const& scope) const
{
    if (scope.empty() || scope[0] != '$')
        return scope;
    size_t slash = scope.find('/');
    std::string name = scope.substr(0, slash);
    auto it = roots_.find(name);
    if (it == roots_.end())
        return scope;
    std::string base = it->second;
    for (size_t p; (p = base.find("{app}")) != std::string::npos;)
        base.replace(p, 5, principal);
    return slash == std::string::npos ? base : base + scope.substr(slash);
}

void Broker::setRoot(std::string const& symbol, std::string const& tmpl)
{
    std::lock_guard l(mu_);
    if (symbol.size() > 1 && symbol[0] == '$' && !tmpl.empty() && tmpl[0] == '/')
        roots_[symbol] = tmpl;
}

namespace {
Permission resolved(Broker const& b, std::string const& principal, Permission p)
{
    auto const* spec = findSpec(p.key);
    if (spec && spec->scope == ScopeKind::Path)
        p.scope = b.resolveRoot(principal, p.scope);
    return p;
}
} // namespace

bool Broker::registerPrincipal(Principal p, PermissionSet declared, std::string& error)
{
    std::lock_guard l(mu_);
    if (p.id.empty() || p.id.size() > 128 || std::any_of(p.id.begin(), p.id.end(), [](unsigned char c) { return c < 0x21 || c == 0x7f; })) {
        error = "invalid principal id";
        return false;
    }
    if (principals_.count(p.id)) {
        error = "principal already registered";
        return false;
    }
    std::string id = p.id;
    principals_[id] = { std::move(p), std::move(declared), false };
    log("principal", id, "", "registered", "declared-sha256=" + principals_[id].declared.digestHex());
    return true;
}

bool Broker::grantActive(Grant const& g) const
{
    if (g.revoked || (g.lifetime == Lifetime::Once && g.used))
        return false;
    if (g.lifetime == Lifetime::Until && clock_() >= g.expires)
        return false;
    return true;
}

RevokeResult Broker::revokeWhere(std::function<bool(Grant const&)> pred, std::string const& by, std::string const& why)
{
    RevokeResult r;
    for (auto& g : grants_) {
        if (g.revoked || !pred(g))
            continue;
        g.revoked = true;
        r.revoked++;
        r.needsProcessRestart |= g.projected;
        log("revoke", g.principal, g.permission.canonical(), "revoked", "grant=" + std::to_string(g.id) + " by=" + by + " " + why);
    }
    return r;
}

RevokeResult Broker::updateDeclared(std::string const& principal, PermissionSet declared, std::string const& by)
{
    std::lock_guard l(mu_);
    auto it = principals_.find(principal);
    if (it == principals_.end())
        return {};
    it->second.declared = std::move(declared);
    log("principal", principal, "", "declared-updated", "declared-sha256=" + it->second.declared.digestHex());
    // Anything granted that the new ceiling no longer covers is withdrawn at once.
    return revokeWhere(
        [&](Grant const& g) {
            if (g.principal != principal)
                return false;
            auto want = resolved(*this, principal, g.permission);
            for (auto const& d : it->second.declared.items())
                if (covers(resolved(*this, principal, d), want))
                    return false;
            return true;
        },
        by, "no longer declared");
}

bool Broker::grantValid(State const& st, Permission const& perm, Lifetime lt, std::string& error) const
{
    std::string perr;
    auto reparsed = parsePermission(perm.canonical(), perr);
    if (!reparsed || !(*reparsed == perm)) {
        error = "invalid permission: " + perr;
        return false;
    }
    if (st.suspended) {
        error = "principal is suspended";
        return false;
    }
    auto want = resolved(*this, st.principal.id, perm);
    bool declared = false;
    for (auto const& d : st.declared.items())
        declared |= covers(resolved(*this, st.principal.id, d), want);
    if (!declared) {
        error = "not declared by the principal's manifest";
        return false;
    }
    Risk risk = riskOf(perm);
    if (risk == Risk::Critical && lt != Lifetime::Once) {
        error = "critical permissions can only be granted once, per use";
        return false;
    }
    if (st.principal.kind == PrincipalKind::Agent) {
        if (isBroad(perm) && lt != Lifetime::Once) {
            error = "broad scopes can only be granted once to an agent";
            return false;
        }
        if (risk >= Risk::High && lt == Lifetime::Standing) {
            error = "standing grants are not allowed for high-risk permissions to an agent";
            return false;
        }
    }
    return true;
}

std::optional<uint64_t> Broker::grant(std::string const& principal, Permission perm, Lifetime lt, int64_t ttl, std::string const& by, std::string& error)
{
    std::lock_guard l(mu_);
    auto it = principals_.find(principal);
    if (it == principals_.end()) {
        error = "unknown principal";
        return std::nullopt;
    }
    if (by.empty()) {
        error = "a grant needs an authorising user";
        return std::nullopt;
    }
    if (!grantValid(it->second, perm, lt, error))
        return std::nullopt;
    if (lt == Lifetime::Until) {
        if (ttl <= 0 || (it->second.principal.kind == PrincipalKind::Agent && ttl > kAgentMaxUntilSeconds)) {
            error = "invalid or too long lifetime";
            return std::nullopt;
        }
    }
    Grant g;
    g.id = nextGrant_++;
    g.principal = principal;
    g.permission = std::move(perm);
    g.lifetime = lt;
    g.created = clock_();
    g.expires = lt == Lifetime::Until ? g.created + ttl : 0;
    g.grantedBy = by;
    log("grant", principal, g.permission.canonical(), "granted", "grant=" + std::to_string(g.id) + " by=" + by + " lifetime=" + std::to_string(static_cast<int>(lt)));
    grants_.push_back(g);
    return g.id;
}

Decision Broker::request(Request const& r)
{
    std::lock_guard l(mu_);
    Decision d;
    try {
        d = requestLocked(r);
    } catch (...) {
        d = { Outcome::Deny, Reason::InternalError, "internal error; denied", 0, 0 };
    }
    std::string res = r.resource.substr(0, 300);
    log("request", r.principal, r.permission + (r.resource.empty() ? "" : ":" + res), toString(d.outcome),
        std::string(toString(d.reason)) + (d.grantId ? " grant=" + std::to_string(d.grantId) : "") + " why=" + r.justification.substr(0, 120));
    return d;
}

Decision Broker::requestLocked(Request const& r)
{
    auto deny = [](Reason why, std::string detail) { return Decision { Outcome::Deny, why, std::move(detail), 0, 0 }; };
    auto it = principals_.find(r.principal);
    if (it == principals_.end())
        return deny(Reason::UnknownPrincipal, "unknown principal");
    State& st = it->second;
    if (st.suspended)
        return deny(Reason::Suspended, "principal is suspended");
    std::string perr;
    auto want = parsePermission(r.resource.empty() ? r.permission : r.permission + ":" + r.resource, perr);
    if (!want)
        return deny(Reason::InvalidRequest, perr);
    Permission wantR = resolved(*this, r.principal, *want);
    bool declared = false;
    for (auto const& d : st.declared.items())
        declared |= covers(resolved(*this, r.principal, d), wantR);
    if (!declared)
        return deny(Reason::NotDeclared, "not in the declared permissions");
    for (auto& g : grants_) {
        if (g.principal != r.principal || !grantActive(g))
            continue;
        if (!covers(resolved(*this, r.principal, g.permission), wantR))
            continue;
        if (g.lifetime == Lifetime::Once)
            g.used = true;
        return { Outcome::Allow, Reason::Granted, "", g.id, 0 };
    }
    if (riskOf(*want) == Risk::Low)
        return { Outcome::Allow, Reason::AutoLowRisk, "", 0, 0 };
    size_t open = 0;
    for (auto& a : approvals_) {
        if (!a.open || a.principal != r.principal)
            continue;
        open++;
        if (resolved(*this, r.principal, a.requested) == wantR)
            return { Outcome::NeedsApproval, Reason::ApprovalRequired, "approval already pending", 0, a.id };
    }
    if (open >= kMaxPendingPerPrincipal)
        return deny(Reason::TooManyPending, "too many pending approvals");
    Approval a;
    a.id = nextApproval_++;
    a.principal = r.principal;
    a.requested = *want;
    a.justification = r.justification.substr(0, 200);
    a.created = clock_();
    approvals_.push_back(a);
    log("approval", r.principal, want->canonical(), "opened", "approval=" + std::to_string(a.id));
    return { Outcome::NeedsApproval, Reason::ApprovalRequired, "approval required", 0, a.id };
}

bool Broker::resolveApproval(uint64_t id, bool approve, Lifetime lt, int64_t ttl, std::string const& by, std::string& error)
{
    Approval* ap = nullptr;
    Permission perm;
    std::string principal;
    {
        std::lock_guard l(mu_);
        for (auto& a : approvals_)
            if (a.id == id && a.open)
                ap = &a;
        if (!ap) {
            error = "no such open approval";
            return false;
        }
        if (by.empty()) {
            error = "an approval needs an authorising user";
            return false;
        }
        ap->open = false;
        perm = ap->requested;
        principal = ap->principal;
        log("approval", principal, perm.canonical(), approve ? "approved" : "denied", "approval=" + std::to_string(id) + " by=" + by);
    }
    if (!approve)
        return true;
    return grant(principal, perm, lt, ttl, by, error).has_value();
}

RevokeResult Broker::revokeGrant(uint64_t id, std::string const& by)
{
    std::lock_guard l(mu_);
    return revokeWhere([&](Grant const& g) { return g.id == id; }, by, "explicit");
}

RevokeResult Broker::revokeAll(std::string const& principal, std::string const& by)
{
    std::lock_guard l(mu_);
    for (auto& a : approvals_)
        if (a.principal == principal)
            a.open = false;
    return revokeWhere([&](Grant const& g) { return g.principal == principal; }, by, "revoke-all");
}

void Broker::suspend(std::string const& principal, std::string const& reason)
{
    std::lock_guard l(mu_);
    auto it = principals_.find(principal);
    if (it == principals_.end())
        return;
    it->second.suspended = true;
    for (auto& a : approvals_)
        if (a.principal == principal)
            a.open = false;
    log("suspend", principal, "", "suspended", reason);
}

void Broker::resume(std::string const& principal, std::string const& by)
{
    std::lock_guard l(mu_);
    auto it = principals_.find(principal);
    if (it == principals_.end() || by.empty())
        return;
    it->second.suspended = false;
    log("suspend", principal, "", "resumed", "by=" + by);
}

RevokeResult Broker::onProcessExit(std::string const& principal, bool crashed)
{
    std::lock_guard l(mu_);
    for (auto& a : approvals_)
        if (a.principal == principal)
            a.open = false;
    log("exit", principal, "", crashed ? "crashed" : "exited", "");
    return revokeWhere([&](Grant const& g) { return g.principal == principal && (g.lifetime == Lifetime::Once || g.lifetime == Lifetime::Session); }, "system",
        "process ended");
}

void Broker::markProjected(std::string const& principal)
{
    std::lock_guard l(mu_);
    for (auto& g : grants_)
        if (g.principal == principal && grantActive(g))
            g.projected = true;
}

std::vector<Grant> Broker::activeGrants(std::string const& principal) const
{
    std::lock_guard l(mu_);
    std::vector<Grant> v;
    for (auto const& g : grants_)
        if (g.principal == principal && grantActive(g))
            v.push_back(g);
    return v;
}

std::vector<Approval> Broker::pendingApprovals() const
{
    std::lock_guard l(mu_);
    std::vector<Approval> v;
    for (auto const& a : approvals_)
        if (a.open)
            v.push_back(a);
    return v;
}

std::vector<AuditEntry> Broker::audit() const
{
    std::lock_guard l(mu_);
    return audit_;
}

std::string Broker::auditText() const
{
    std::lock_guard l(mu_);
    std::string out;
    for (auto const& e : audit_)
        out += line(e) + "|" + e.hash + "\n";
    return out;
}

std::string Broker::auditHead() const
{
    std::lock_guard l(mu_);
    return audit_.empty() ? kGenesis : audit_.back().hash;
}

AuditVerdict Broker::verifyAuditText(std::string const& text)
{
    AuditVerdict v;
    std::string prev = kGenesis;
    std::istringstream in(text);
    std::string ln;
    size_t n = 0;
    while (std::getline(in, ln)) {
        n++;
        size_t cut = ln.rfind('|');
        if (cut == std::string::npos || ln.size() - cut - 1 != 64) {
            v.firstBadLine = n, v.problem = "malformed line";
            return v;
        }
        std::string body = ln.substr(0, cut), hash = ln.substr(cut + 1);
        if (std::count(body.begin(), body.end(), '|') != 6) {
            v.firstBadLine = n, v.problem = "malformed fields";
            return v;
        }
        if (body.substr(0, body.find('|')) != std::to_string(n)) {
            v.firstBadLine = n, v.problem = "sequence gap or reorder";
            return v;
        }
        if (chain(prev, body) != hash) {
            v.firstBadLine = n, v.problem = "hash mismatch (entry altered, removed or reordered)";
            return v;
        }
        prev = hash;
    }
    v.ok = true;
    v.entries = n;
    return v;
}

} // namespace exile::policy
