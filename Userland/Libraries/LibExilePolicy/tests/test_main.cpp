/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Host tests for LibExilePolicy. They test policy logic only; they say nothing about kernel enforcement.

#include <atomic>
#include <fstream>
#include <functional>
#include <iostream>
#include <set>
#include <sstream>
#include <thread>

#include "broker.h"
#include "kernel_map.h"
#include "permission.h"
#include "sha256.h"

using namespace exile::policy;

namespace {
struct Case {
    char const* name;
    std::function<void()> fn;
};
std::vector<Case>& cases()
{
    static std::vector<Case> c;
    return c;
}
struct Reg {
    Reg(char const* n, std::function<void()> f) { cases().push_back({ n, std::move(f) }); }
};
int g_fail = 0;
#define TEST(name)                       \
    static void name();                  \
    static Reg reg_##name(#name, name);  \
    static void name()
#define CHECK(c)                                                                      \
    do {                                                                              \
        if (!(c)) {                                                                   \
            std::cerr << "  CHECK failed: " #c " (" << __FILE__ << ":" << __LINE__ << ")\n"; \
            g_fail++;                                                                 \
        }                                                                             \
    } while (0)

Permission P(std::string t)
{
    std::string e;
    auto p = parsePermission(t, e);
    if (!p) {
        std::cerr << "  bad test permission " << t << ": " << e << "\n";
        g_fail++;
        return {};
    }
    return *p;
}
PermissionSet S(std::vector<std::string> v)
{
    std::string e;
    auto s = PermissionSet::parse(v, e);
    if (!s) {
        std::cerr << "  bad set: " << e << "\n";
        g_fail++;
        return {};
    }
    return *s;
}

struct Fx {
    int64_t now = 1000;
    Broker b { [this] { return now; } };
    std::string err;
    Fx()
    {
        b.setRoot("$documents", "/home/anon/Documents");
        b.setRoot("$app-data", "/home/anon/.local/share/{app}");
        CHECK(b.registerPrincipal({ "org.x.notes", PrincipalKind::App },
            S({ "fs.read:$documents", "fs.write:$app-data", "net.connect:api.example.com:443", "ui.notify", "sys.settings:display", "dev.camera" }), err));
        CHECK(b.registerPrincipal({ "eis.agent", PrincipalKind::Agent },
            S({ "fs.read:/home/anon/Documents", "fs.write:/home/anon/Documents", "proc.spawn:/usr/bin/tool", "net.connect:*.example.com:443", "ai.cloud:llm.example.com:443", "sys.settings:*" }), err));
    }
};
} // namespace

// ---------------------------------------------------------------- permission model
TEST(path_normalisation)
{
    CHECK(normalizePath("/a/b").value_or("") == "/a/b");
    CHECK(normalizePath("/a/b/").value_or("") == "/a/b");
    CHECK(normalizePath("/").value_or("") == "/");
    CHECK(normalizePath("$documents/x").value_or("") == "$documents/x");
    CHECK(normalizePath("$documents").value_or("") == "$documents");
    for (char const* bad : { "", "a/b", "/a/../b", "/a/./b", "/a//b", "/..", "/a\\b", "$", "$Docs", "/a\x01", "~/x" })
        CHECK(!normalizePath(bad));
    CHECK(!normalizePath(std::string("/a\0b", 4)));
    CHECK(!normalizePath("/" + std::string(300, 'a')));
    CHECK(!normalizePath("/" + std::string(5000, 'a') + "/b"));
}

TEST(host_normalisation)
{
    CHECK(normalizeHostPort("Example.COM:443").value_or("") == "example.com:443");
    CHECK(normalizeHostPort("*.example.com:*").value_or("") == "*.example.com:*");
    CHECK(normalizeHostPort("*:443").value_or("") == "*:443");
    for (char const* bad : { "example.com", ":443", "example.com:0", "example.com:65536", "example.com:0443", "exa mple.com:1", "-a.com:1", "a..com:1", "*.:1", "example.com:x", "a.com:1:2" })
        CHECK(!normalizeHostPort(bad));
}

TEST(parse_is_closed_and_strict)
{
    std::string e;
    CHECK(!parsePermission("fs.exec:/a", e) && e.find("unknown") != std::string::npos);
    CHECK(!parsePermission("fs.read", e));          // scope required
    CHECK(!parsePermission("ui.notify:x", e));      // scope forbidden
    CHECK(!parsePermission("proc.spawn:/", e));     // exec must name one program
    CHECK(!parsePermission("proc.spawn:$documents/x", e));
    CHECK(!parsePermission("sys.settings:bad key", e));
    CHECK(!parsePermission("net.connect:*", e));
    CHECK(parsePermission("sys.settings:display.brightness", e));
    CHECK(!parsePermission("", e));
}

TEST(scope_coverage_is_component_aware)
{
    CHECK(covers(P("fs.read:/home/a"), P("fs.read:/home/a")));
    CHECK(covers(P("fs.read:/home/a"), P("fs.read:/home/a/b/c")));
    CHECK(!covers(P("fs.read:/home/a"), P("fs.read:/home/ab"))); // sibling with shared prefix
    CHECK(!covers(P("fs.read:/home/a/b"), P("fs.read:/home/a")));
    CHECK(!covers(P("fs.read:/home/a"), P("fs.write:/home/a")));
    CHECK(covers(P("fs.read:/"), P("fs.read:/anything")));
    CHECK(covers(P("net.connect:*.example.com:443"), P("net.connect:api.example.com:443")));
    CHECK(!covers(P("net.connect:*.example.com:443"), P("net.connect:example.com:443")));
    CHECK(!covers(P("net.connect:*.example.com:443"), P("net.connect:evilexample.com:443")));
    CHECK(!covers(P("net.connect:*.example.com:443"), P("net.connect:api.example.com:80")));
    CHECK(covers(P("net.connect:api.example.com:*"), P("net.connect:api.example.com:80")));
    CHECK(!covers(P("proc.spawn:/usr/bin/a"), P("proc.spawn:/usr/bin/ab")));
    CHECK(covers(P("sys.settings:*"), P("sys.settings:x")));
    CHECK(!covers(P("sys.settings:x"), P("sys.settings:y")));
    CHECK(covers(P("ui.notify"), P("ui.notify")));
}

TEST(broadness)
{
    CHECK(isBroad(P("fs.read:/")));
    CHECK(isBroad(P("fs.read:$documents")));
    CHECK(!isBroad(P("fs.read:$documents/a")));
    CHECK(isBroad(P("net.connect:*.example.com:443")));
    CHECK(isBroad(P("net.connect:a.com:*")));
    CHECK(!isBroad(P("net.connect:a.com:443")));
    CHECK(isBroad(P("sys.settings:*")));
}

TEST(set_is_canonical_and_digest_binds_content)
{
    auto a = S({ "ui.notify", "fs.read:/b", "fs.read:/a", "ui.notify" });
    auto b = S({ "fs.read:/a", "fs.read:/b", "ui.notify" });
    CHECK(a.canonicalText() == b.canonicalText());
    CHECK(a.digestHex() == b.digestHex());
    CHECK(a.digestHex().size() == 64);
    CHECK(a.digestHex() != S({ "fs.read:/a", "ui.notify" }).digestHex());
}

TEST(update_escalation_is_detected)
{
    auto v1 = S({ "fs.read:/home/a", "ui.notify" });
    CHECK(v1.escalationTo(S({ "fs.read:/home/a/sub", "ui.notify" })).empty()); // narrowing is fine
    auto e = v1.escalationTo(S({ "fs.read:/home/a", "ui.notify", "dev.camera", "fs.write:/home/a" }));
    CHECK(e.size() == 2);
    CHECK(!v1.escalationTo(S({ "fs.read:/home" })).empty()); // widening a path is escalation
}

TEST(legacy_ids_map_and_unknown_is_never_dropped)
{
    auto m = mapLegacyIds({ "camera", "documents", "network", "made-up", "system-settings" });
    CHECK(m.set.allows(P("dev.camera")));
    CHECK(m.set.allows(P("fs.write:$documents/x")));
    CHECK(m.set.allows(P("net.connect:any.host.org:443")));
    CHECK(m.set.allows(P("sys.settings:anything")));
    CHECK(m.unmapped == std::vector<std::string> { "made-up" });
    // every Store vocabulary id maps
    auto all = mapLegacyIds({ "network", "camera", "microphone", "location", "documents", "photos", "contacts", "notifications", "background", "bluetooth", "usb", "clipboard", "system-settings" });
    CHECK(all.unmapped.empty());
}

TEST(signing_payload_binds_permissions)
{
    auto p1 = S({ "ui.notify" }), p2 = S({ "ui.notify", "dev.camera" });
    auto a = signingPayloadV2("org.x", "1.0.0", "pkg", std::string(64, 'a'), 10, p1);
    auto b = signingPayloadV2("org.x", "1.0.0", "pkg", std::string(64, 'a'), 10, p2);
    CHECK(a != b);
    CHECK(a == signingPayloadV2("org.x", "1.0.0", "pkg", std::string(64, 'a'), 10, p1));
    CHECK(a.rfind("exile-package-signature-v2\n", 0) == 0);
    CHECK(a != signingPayloadV2("org.x", "1.0.1", "pkg", std::string(64, 'a'), 10, p1));
}

// ---------------------------------------------------------------- broker
TEST(default_deny)
{
    Fx f;
    CHECK(f.b.request({ "ghost", "fs.read", "/home/anon/Documents/a", "" }).reason == Reason::UnknownPrincipal);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "/etc/shadow", "" }).reason == Reason::NotDeclared);
    CHECK(f.b.request({ "org.x.notes", "fs.exec", "/bin/sh", "" }).reason == Reason::InvalidRequest);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "", "" }).outcome == Outcome::Deny);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "../../etc", "" }).reason == Reason::InvalidRequest);
}

TEST(declared_ceiling_beats_user_approval)
{
    Fx f;
    std::string e;
    CHECK(!f.b.grant("org.x.notes", P("fs.read:/etc"), Lifetime::Standing, 0, "user:anon", e));
    CHECK(e.find("not declared") != std::string::npos);
    CHECK(!f.b.grant("org.x.notes", P("fs.read:$documents"), Lifetime::Standing, 0, "", e)); // needs an authorising user
}

TEST(request_then_approval_then_use)
{
    Fx f;
    std::string e;
    auto d = f.b.request({ "org.x.notes", "fs.read", "/home/anon/Documents/a.txt", "open a note" });
    CHECK(d.outcome == Outcome::NeedsApproval && d.approvalId != 0);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "/home/anon/Documents/a.txt", "" }).approvalId == d.approvalId); // dedup
    CHECK(f.b.pendingApprovals().size() == 1);
    CHECK(f.b.resolveApproval(d.approvalId, true, Lifetime::Session, 0, "user:anon", e));
    CHECK(f.b.request({ "org.x.notes", "fs.read", "/home/anon/Documents/a.txt", "" }).outcome == Outcome::Allow);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "/home/anon/Documents/other", "" }).outcome == Outcome::NeedsApproval); // grant was exact
    CHECK(!f.b.resolveApproval(d.approvalId, true, Lifetime::Session, 0, "user:anon", e)); // already resolved
}

TEST(denied_approval_stays_denied_until_asked_again)
{
    Fx f;
    std::string e;
    auto d = f.b.request({ "org.x.notes", "dev.camera", "", "scan" });
    CHECK(f.b.resolveApproval(d.approvalId, false, Lifetime::Once, 0, "user:anon", e));
    CHECK(f.b.activeGrants("org.x.notes").empty());
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::NeedsApproval);
}

TEST(once_grants_are_single_use)
{
    Fx f;
    std::string e;
    CHECK(f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Once, 0, "user:anon", e));
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::Allow);
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::NeedsApproval);
}

TEST(time_limited_grants_expire)
{
    Fx f;
    std::string e;
    CHECK(f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Until, 60, "user:anon", e));
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::Allow);
    f.now += 59;
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::Allow);
    f.now += 1;
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::NeedsApproval);
    CHECK(!f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Until, 0, "user:anon", e));
}

TEST(revocation_is_immediate)
{
    Fx f;
    std::string e;
    auto id = f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Standing, 0, "user:anon", e);
    CHECK(id && f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::Allow);
    CHECK(f.b.revokeGrant(*id, "user:anon").revoked == 1);
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::NeedsApproval);
    CHECK(f.b.revokeGrant(*id, "user:anon").revoked == 0); // idempotent
}

TEST(revoke_all_closes_pending_too)
{
    Fx f;
    std::string e;
    f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Standing, 0, "user:anon", e);
    f.b.request({ "org.x.notes", "fs.read", "$documents/q", "" });
    CHECK(f.b.revokeAll("org.x.notes", "user:anon").revoked == 1);
    CHECK(f.b.pendingApprovals().empty());
}

TEST(critical_permissions_are_per_use_only)
{
    Fx f;
    std::string e;
    CHECK(!f.b.grant("org.x.notes", P("sys.settings:display"), Lifetime::Standing, 0, "user:anon", e));
    CHECK(!f.b.grant("org.x.notes", P("sys.settings:display"), Lifetime::Session, 0, "user:anon", e));
    CHECK(f.b.grant("org.x.notes", P("sys.settings:display"), Lifetime::Once, 0, "user:anon", e));
}

TEST(agent_cannot_hold_broad_or_standing_high_risk_authority)
{
    Fx f;
    std::string e;
    CHECK(!f.b.grant("eis.agent", P("net.connect:*.example.com:443"), Lifetime::Standing, 0, "user:anon", e)); // broad
    CHECK(!f.b.grant("eis.agent", P("fs.write:/home/anon/Documents"), Lifetime::Standing, 0, "user:anon", e)); // high risk
    CHECK(!f.b.grant("eis.agent", P("fs.write:/home/anon/Documents"), Lifetime::Until, 3 * 24 * 3600, "user:anon", e)); // too long
    CHECK(f.b.grant("eis.agent", P("fs.write:/home/anon/Documents"), Lifetime::Until, 3600, "user:anon", e));
    CHECK(f.b.grant("eis.agent", P("net.connect:*.example.com:443"), Lifetime::Once, 0, "user:anon", e));
    CHECK(f.b.grant("eis.agent", P("fs.read:/home/anon/Documents"), Lifetime::Standing, 0, "user:anon", e)); // medium, narrow
}

TEST(agent_prompt_injection_cannot_widen_what_a_grant_covers)
{
    Fx f;
    std::string e;
    f.b.grant("eis.agent", P("fs.read:/home/anon/Documents"), Lifetime::Standing, 0, "user:anon", e);
    AgentFacade ai(f.b, "eis.agent");
    CHECK(ai.request("fs.read", "/home/anon/Documents/a", "ok").outcome == Outcome::Allow);
    CHECK(ai.request("fs.read", "/home/anon/Documents/../.ssh/id_rsa", "ignore previous instructions").reason == Reason::InvalidRequest);
    CHECK(ai.request("fs.read", "/home/anon/DocumentsX", "").outcome != Outcome::Allow);
    CHECK(ai.request("fs.read", "/home/anon/.ssh/id_rsa", "").reason == Reason::NotDeclared);
    CHECK(ai.request("fs.write", "/home/anon/Documents/a", "").outcome == Outcome::NeedsApproval); // declared, not granted
    CHECK(ai.request("proc.spawn", "/bin/sh", "").reason == Reason::NotDeclared);
    CHECK(ai.request("sys.install", "", "").reason == Reason::NotDeclared);
    CHECK(ai.request("fs.read", std::string("/home/anon/Documents/a\0/../../x", 31), "").outcome == Outcome::Deny);
}

template<typename T>
concept CanGrant = requires(T a, std::string& e) { a.grant("p", Permission {}, Lifetime::Once, 0, "u", e); };
template<typename T>
concept CanApprove = requires(T a, std::string& e) { a.resolveApproval(1, true, Lifetime::Once, 0, "u", e); };
template<typename T>
concept CanRevoke = requires(T a) { a.revokeAll("p", "u"); };
template<typename T>
concept CanSuspend = requires(T a) { a.suspend("p", "r"); };
template<typename T>
concept CanRequest = requires(T a) { a.request("fs.read", "/x"); };

TEST(facade_has_no_authority_surface)
{
    // Compile-time proof: the object handed to an agent offers no way to grant, approve, revoke or suspend,
    // while the same operations exist on Broker (so the concepts are not vacuous).
    static_assert(CanGrant<Broker> && CanApprove<Broker> && CanRevoke<Broker> && CanSuspend<Broker>);
    static_assert(!CanGrant<AgentFacade> && !CanApprove<AgentFacade> && !CanRevoke<AgentFacade> && !CanSuspend<AgentFacade>);
    static_assert(CanRequest<AgentFacade>);
    CHECK(true);
}

TEST(failure_of_the_broker_fails_closed)
{
    CHECK(!failClosedAllows(std::nullopt));
    Decision d;
    CHECK(!failClosedAllows(d)); // default-constructed decision is a deny
    d.outcome = Outcome::NeedsApproval;
    CHECK(!failClosedAllows(d));
    d.outcome = Outcome::Allow;
    CHECK(failClosedAllows(d));
}

TEST(low_risk_is_auto_allowed_only_when_declared)
{
    Fx f;
    CHECK(f.b.request({ "org.x.notes", "ui.notify", "", "" }).reason == Reason::AutoLowRisk);
    CHECK(f.b.request({ "eis.agent", "ui.notify", "", "" }).reason == Reason::NotDeclared);
}

TEST(symbolic_roots_resolve_per_principal)
{
    Fx f;
    std::string e;
    f.b.grant("org.x.notes", P("fs.write:$app-data"), Lifetime::Standing, 0, "user:anon", e);
    CHECK(f.b.request({ "org.x.notes", "fs.write", "/home/anon/.local/share/org.x.notes/db", "" }).outcome == Outcome::Allow);
    CHECK(f.b.request({ "org.x.notes", "fs.write", "/home/anon/.local/share/other.app/db", "" }).outcome != Outcome::Allow);
    CHECK(f.b.request({ "org.x.notes", "fs.write", "$app-data/db", "" }).outcome == Outcome::Allow);
}

TEST(update_withdraws_grants_no_longer_declared)
{
    Fx f;
    std::string e;
    f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Standing, 0, "user:anon", e);
    f.b.grant("org.x.notes", P("fs.read:$documents/a"), Lifetime::Standing, 0, "user:anon", e);
    auto r = f.b.updateDeclared("org.x.notes", S({ "fs.read:$documents", "ui.notify" }), "xam-update");
    CHECK(r.revoked == 1);
    CHECK(f.b.request({ "org.x.notes", "dev.camera", "", "" }).reason == Reason::NotDeclared);
    CHECK(f.b.request({ "org.x.notes", "fs.read", "$documents/a", "" }).outcome == Outcome::Allow);
}

TEST(suspension_and_process_exit_contain_failures)
{
    Fx f;
    std::string e;
    f.b.grant("eis.agent", P("fs.read:/home/anon/Documents"), Lifetime::Session, 0, "user:anon", e);
    f.b.grant("eis.agent", P("proc.spawn:/usr/bin/tool"), Lifetime::Until, 600, "user:anon", e);
    f.b.request({ "eis.agent", "fs.write", "/home/anon/Documents/z", "" });
    f.b.suspend("eis.agent", "anomalous request rate");
    CHECK(f.b.request({ "eis.agent", "fs.read", "/home/anon/Documents/a", "" }).reason == Reason::Suspended);
    CHECK(f.b.pendingApprovals().empty());
    CHECK(!f.b.grant("eis.agent", P("fs.read:/home/anon/Documents"), Lifetime::Once, 0, "user:anon", e));
    f.b.resume("eis.agent", "user:anon");
    auto r = f.b.onProcessExit("eis.agent", true);
    CHECK(r.revoked == 1); // the Session grant ends with the process; the time-limited one survives a restart
    CHECK(f.b.request({ "eis.agent", "fs.read", "/home/anon/Documents/a", "" }).outcome == Outcome::NeedsApproval);
    CHECK(f.b.request({ "eis.agent", "proc.spawn", "/usr/bin/tool", "" }).outcome == Outcome::Allow);
}

TEST(pending_approvals_are_bounded)
{
    Fx f;
    int denied = 0;
    for (int i = 0; i < 80; i++)
        if (f.b.request({ "eis.agent", "fs.write", "/home/anon/Documents/f" + std::to_string(i), "" }).reason == Reason::TooManyPending)
            denied++;
    CHECK(denied == 80 - 32);
    CHECK(f.b.pendingApprovals().size() == 32);
}

TEST(principal_registration_is_validated)
{
    Fx f;
    std::string e;
    CHECK(!f.b.registerPrincipal({ "org.x.notes", PrincipalKind::App }, {}, e)); // duplicate
    CHECK(!f.b.registerPrincipal({ "", PrincipalKind::App }, {}, e));
    CHECK(!f.b.registerPrincipal({ "has space", PrincipalKind::App }, {}, e));
    CHECK(!f.b.registerPrincipal({ "new\nline", PrincipalKind::App }, {}, e));
}

// ---------------------------------------------------------------- audit
TEST(audit_records_everything_and_verifies)
{
    Fx f;
    std::string e;
    f.b.request({ "org.x.notes", "dev.camera", "", "scan" });
    f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Once, 0, "user:anon", e);
    f.b.request({ "org.x.notes", "dev.camera", "", "" });
    f.b.revokeAll("org.x.notes", "user:anon");
    auto text = f.b.auditText();
    auto v = Broker::verifyAuditText(text);
    CHECK(v.ok && v.entries == f.b.audit().size());
    CHECK(text.find("|request|") != std::string::npos && text.find("|grant|") != std::string::npos);
    CHECK(Broker::verifyAuditText("").ok);
}

TEST(audit_detects_tampering)
{
    Fx f;
    std::string e;
    for (int i = 0; i < 6; i++)
        f.b.request({ "org.x.notes", "dev.camera", "", "n" + std::to_string(i) });
    std::string text = f.b.auditText();
    // edit
    auto edited = text;
    auto p = edited.find("needs-approval");
    edited.replace(p, 14, "allow");
    CHECK(!Broker::verifyAuditText(edited).ok);
    // delete a middle line
    std::istringstream in(text);
    std::vector<std::string> lines;
    for (std::string l; std::getline(in, l);)
        lines.push_back(l);
    auto join = [](std::vector<std::string> const& v) { std::string s; for (auto& l : v) s += l + "\n"; return s; };
    auto del = lines;
    del.erase(del.begin() + 3);
    auto vd = Broker::verifyAuditText(join(del));
    CHECK(!vd.ok && vd.firstBadLine == 4);
    // reorder
    auto ro = lines;
    std::swap(ro[2], ro[3]);
    CHECK(!Broker::verifyAuditText(join(ro)).ok);
    // tail truncation is NOT detectable from the text alone; the head anchor is what catches it
    auto trunc = lines;
    trunc.pop_back();
    CHECK(Broker::verifyAuditText(join(trunc)).ok);
    CHECK(f.b.auditHead() != Broker::verifyAuditText(join(trunc)).problem); // documented limitation: anchor externally
}

TEST(audit_chain_catches_a_forger_who_recomputes_the_edited_entry_hash)
{
    Fx f;
    for (int i = 0; i < 5; i++)
        f.b.request({ "org.x.notes", "dev.camera", "", "n" + std::to_string(i) });
    std::istringstream in(f.b.auditText());
    std::vector<std::string> lines;
    for (std::string l; std::getline(in, l);)
        lines.push_back(l);
    // Rewrite one entry and give it a correct hash for its own position (anyone can do this: SHA-256 is unkeyed).
    size_t k = 0;
    while (k < lines.size() && lines[k].find("|needs-approval|") == std::string::npos)
        k++;
    CHECK(k > 0 && k + 1 < lines.size());
    std::string prev = lines[k - 1].substr(lines[k - 1].rfind('|') + 1);
    std::string body = lines[k].substr(0, lines[k].rfind('|'));
    auto p = body.find("needs-approval");
    body.replace(p, 14, "allow");
    lines[k] = body + "|" + exile::sha256Hex(prev + "\n" + body);
    std::string text;
    for (auto& l : lines)
        text += l + "\n";
    auto v = Broker::verifyAuditText(text);
    CHECK(!v.ok && v.firstBadLine == k + 2); // the NEXT entry no longer chains to the forged one
}

TEST(audit_neutralises_hostile_text)
{
    Fx f;
    f.b.request({ "org.x.notes", "dev.camera", "", "line1\nFAKE|grant|x|y|allow|z\r\n" + std::string(5000, 'A') });
    auto text = f.b.auditText();
    CHECK(Broker::verifyAuditText(text).ok);
    size_t lines = std::count(text.begin(), text.end(), '\n');
    CHECK(lines == f.b.audit().size()); // the newline in the justification did not create a line
    CHECK(text.find("FAKE|grant") == std::string::npos);
    CHECK(text.size() < 2000); // justification truncated
}

// ---------------------------------------------------------------- kernel projection
static std::set<std::string> realPledgePromises()
{
    std::ifstream in(std::string(EXILE_REPO_ROOT) + "/Kernel/Tasks/Process.h");
    std::set<std::string> out;
    std::string l;
    bool in_block = false;
    while (std::getline(in, l)) {
        if (l.find("#define ENUMERATE_PLEDGE_PROMISES") != std::string::npos)
            in_block = true;
        if (!in_block)
            continue;
        auto p = l.find("__ENUMERATE_PLEDGE_PROMISE(");
        if (p != std::string::npos) {
            auto s = p + 27, e = l.find(')', s);
            out.insert(l.substr(s, e - s));
        }
        if (l.find("no_error") != std::string::npos)
            break;
    }
    return out;
}

TEST(emitted_promises_exist_in_the_real_kernel)
{
    auto real = realPledgePromises();
    CHECK(real.size() > 20 && real.count("stdio") && real.count("rpath"));
    Fx f;
    std::string e;
    for (auto p : { "fs.read:$documents", "fs.write:$app-data", "net.connect:api.example.com:443" })
        f.b.grant("org.x.notes", P(p), Lifetime::Standing, 0, "user:anon", e);
    f.b.grant("eis.agent", P("proc.spawn:/usr/bin/tool"), Lifetime::Standing, 0, "user:anon", e);
    for (auto id : { "org.x.notes", "eis.agent" }) {
        auto k = projectGrants(f.b, id);
        CHECK(!k.promises.empty());
        for (auto const& p : k.promises)
            CHECK(real.count(p));
    }
}

TEST(projection_contents)
{
    Fx f;
    std::string e;
    f.b.grant("org.x.notes", P("fs.read:$documents"), Lifetime::Standing, 0, "user:anon", e);
    f.b.grant("org.x.notes", P("fs.write:$app-data"), Lifetime::Session, 0, "user:anon", e);
    f.b.grant("org.x.notes", P("net.connect:api.example.com:443"), Lifetime::Standing, 0, "user:anon", e);
    f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Standing, 0, "user:anon", e);
    f.b.grant("org.x.notes", P("sys.settings:display"), Lifetime::Once, 0, "user:anon", e);
    auto k = projectGrants(f.b, "org.x.notes");
    CHECK(k.promiseString() == "cpath inet rpath stdio unix wpath");
    CHECK(k.unveil.size() == 2);
    CHECK(k.unveil[0].path == "/home/anon/.local/share/org.x.notes" && k.unveil[0].perms == "crw");
    CHECK(k.unveil[1].path == "/home/anon/Documents" && k.unveil[1].perms == "r");
    bool camera = false, net = false;
    for (auto& m : k.brokerMediated) {
        camera |= m == "dev.camera";
        net |= m.find("net.connect") == 0 && m.find("cannot restrict") != std::string::npos;
        CHECK(m.find("sys.settings") == std::string::npos); // Once grants are never ambient authority
    }
    CHECK(camera && net);
}

TEST(no_grants_means_no_filesystem)
{
    Fx f;
    auto k = projectGrants(f.b, "org.x.notes");
    CHECK(k.promiseString() == "stdio");
    CHECK(k.unveil.size() == 1 && k.unveil[0].path == "/" && k.unveil[0].perms.empty());
}

TEST(unresolved_symbols_are_skipped_not_widened)
{
    Fx f;
    std::string e;
    Broker b([] { return 0; });
    CHECK(b.registerPrincipal({ "a", PrincipalKind::App }, S({ "fs.read:$photos" }), e));
    CHECK(b.grant("a", P("fs.read:$photos"), Lifetime::Standing, 0, "u", e));
    auto k = projectGrants(b, "a");
    CHECK(k.promiseString() == "stdio" && k.skipped.size() == 1);
    CHECK(k.unveil.size() == 1 && k.unveil[0].perms.empty());
}

TEST(revoking_projected_authority_requires_a_restart)
{
    Fx f;
    std::string e;
    auto id = f.b.grant("org.x.notes", P("fs.read:$documents"), Lifetime::Standing, 0, "user:anon", e);
    projectAndMark(f.b, "org.x.notes");
    auto r = f.b.revokeGrant(*id, "user:anon");
    CHECK(r.revoked == 1 && r.needsProcessRestart);
    auto id2 = f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Standing, 0, "user:anon", e);
    CHECK(!f.b.revokeGrant(*id2, "user:anon").needsProcessRestart); // never projected
}

// ---------------------------------------------------------------- concurrency smoke
TEST(concurrent_use_is_safe)
{
    Fx f;
    std::atomic<int> allowed { 0 };
    std::vector<std::thread> ts;
    for (int t = 0; t < 6; t++)
        ts.emplace_back([&, t] {
            std::string e;
            for (int i = 0; i < 300; i++) {
                if (t == 0)
                    f.b.grant("org.x.notes", P("dev.camera"), Lifetime::Once, 0, "user:anon", e);
                else if (t == 1)
                    f.b.revokeAll("eis.agent", "user:anon");
                else if (f.b.request({ "org.x.notes", "dev.camera", "", "" }).outcome == Outcome::Allow)
                    allowed++;
            }
        });
    for (auto& t : ts)
        t.join();
    CHECK(allowed <= 300); // each Once grant can be consumed by at most one request
    CHECK(Broker::verifyAuditText(f.b.auditText()).ok);
}

int main()
{
    int ran = 0;
    for (auto& c : cases()) {
        int before = g_fail;
        c.fn();
        std::cout << (g_fail == before ? "[ OK ] " : "[FAIL] ") << c.name << "\n";
        ran++;
    }
    std::cout << ran << " tests, " << g_fail << " failed checks\n";
    return g_fail == 0 && ran > 0 ? 0 : 1;
}
