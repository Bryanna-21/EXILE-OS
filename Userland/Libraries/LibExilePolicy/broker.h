/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// EXILE permission broker: the single decision point for "may this principal do this?".
//
// Design rules (see Documentation/Architecture/CoreIdentity/EIS_PERMISSION_BROKER.md):
//  * Independent of any AI model. An EIS agent is just a Principal of kind Agent; it can only
//    *ask* (AgentFacade). Granting, revoking and approving live on Broker, which an agent
//    process must never be handed.
//  * Default deny. Every error path (unknown principal, malformed input, internal failure) denies.
//  * Declared ceiling: nothing outside the principal's declared (manifest) permissions can be granted.
//  * Critical permissions are per-use (Once) only; Agents never get broad standing grants.
//  * Every request, decision, grant and revocation is appended to a hash-chained audit log.
//
// This library is policy logic only. Enforcement against a running process needs the kernel
// (pledge/unveil via kernel_map.h) and a trusted IPC channel, neither of which is validated here.

#include <cstdint>
#include <functional>
#include <map>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "permission.h"

namespace exile::policy {

enum class PrincipalKind { App, Agent, System };

struct Principal {
    std::string id; // app id, or agent id
    PrincipalKind kind = PrincipalKind::App;
};

enum class Outcome { Allow, Deny, NeedsApproval };
enum class Reason {
    Granted,
    AutoLowRisk,
    UnknownPrincipal,
    Suspended,
    InvalidRequest,
    NotDeclared,
    ApprovalRequired,
    TooManyPending,
    InternalError,
};
char const* toString(Outcome);
char const* toString(Reason);

struct Request {
    std::string principal;
    std::string permission; // "fs.read"
    std::string resource;   // path / host:port / program / settings key; empty for unscoped
    std::string justification; // free text from the requester; logged truncated, never trusted
};

struct Decision {
    Outcome outcome = Outcome::Deny;
    Reason reason = Reason::InternalError;
    std::string detail;
    uint64_t grantId = 0;    // set on Allow via grant
    uint64_t approvalId = 0; // set on NeedsApproval
};

enum class Lifetime { Once, Session, Until, Standing };

struct Grant {
    uint64_t id = 0;
    std::string principal;
    Permission permission;
    Lifetime lifetime = Lifetime::Once;
    int64_t expires = 0; // for Until
    int64_t created = 0;
    std::string grantedBy;
    bool revoked = false;
    bool used = false;      // Once grants
    bool projected = false; // handed to the kernel as ambient pledge/unveil authority
};

struct Approval {
    uint64_t id = 0;
    std::string principal;
    Permission requested;
    std::string justification;
    int64_t created = 0;
    bool open = true;
};

struct AuditEntry {
    uint64_t seq = 0;
    int64_t time = 0;
    std::string kind; // request|grant|revoke|approval|principal|suspend|exit
    std::string principal;
    std::string permission;
    std::string outcome;
    std::string detail;
    std::string prevHash;
    std::string hash;
};

struct RevokeResult {
    size_t revoked = 0;
    bool needsProcessRestart = false; // true if any revoked grant had been projected into the kernel
};

struct AuditVerdict {
    bool ok = false;
    size_t entries = 0;
    size_t firstBadLine = 0; // 1-based, when !ok
    std::string problem;
};

class Broker {
public:
    using Clock = std::function<int64_t()>; // seconds
    explicit Broker(Clock clock);

    // --- principals (trusted configuration path) ---
    bool registerPrincipal(Principal p, PermissionSet declared, std::string& error);
    // Replace the declared ceiling after an update. Grants no longer covered are revoked.
    RevokeResult updateDeclared(std::string const& principal, PermissionSet declared, std::string const& by);
    void setRoot(std::string const& symbol, std::string const& pathTemplate); // "$documents" -> "/home/{user}/Documents"

    // --- the one question ---
    Decision request(Request const& r);

    // --- authority (user-approved path; NOT reachable from AgentFacade) ---
    std::optional<uint64_t> grant(std::string const& principal, Permission perm, Lifetime lt, int64_t ttlSeconds,
        std::string const& by, std::string& error);
    // approve=true creates a grant for the requested permission (Once unless `lt` says otherwise).
    bool resolveApproval(uint64_t approvalId, bool approve, Lifetime lt, int64_t ttlSeconds, std::string const& by, std::string& error);
    RevokeResult revokeGrant(uint64_t grantId, std::string const& by);
    RevokeResult revokeAll(std::string const& principal, std::string const& by);

    // --- containment / recovery ---
    void suspend(std::string const& principal, std::string const& reason);
    void resume(std::string const& principal, std::string const& by);
    // The process died (crash or normal). Session/Once grants end, pending approvals close.
    RevokeResult onProcessExit(std::string const& principal, bool crashed);

    // --- introspection ---
    std::vector<Grant> activeGrants(std::string const& principal) const;
    std::vector<Approval> pendingApprovals() const;
    std::vector<AuditEntry> audit() const;
    std::string auditText() const;      // serialised, one hash-chained line per entry
    std::string auditHead() const;      // anchor this externally to detect tail truncation
    void markProjected(std::string const& principal); // called by kernel_map when authority is handed to the kernel
    std::string resolveRoot(std::string const& principal, std::string const& scope) const;

    static AuditVerdict verifyAuditText(std::string const& text);

private:
    struct State {
        Principal principal;
        PermissionSet declared;
        bool suspended = false;
    };
    Decision requestLocked(Request const& r);
    void log(std::string kind, std::string principal, std::string perm, std::string outcome, std::string detail);
    bool grantValid(State const& st, Permission const& perm, Lifetime lt, std::string& error) const;
    bool grantActive(Grant const& g) const;
    RevokeResult revokeWhere(std::function<bool(Grant const&)> pred, std::string const& by, std::string const& why);

    mutable std::mutex mu_;
    Clock clock_;
    std::map<std::string, State> principals_;
    std::vector<Grant> grants_;
    std::vector<Approval> approvals_;
    std::vector<AuditEntry> audit_;
    std::map<std::string, std::string> roots_;
    uint64_t nextGrant_ = 1, nextApproval_ = 1;
};

// What an AI agent process is given. It can ask and look at its own approval; nothing else.
class AgentFacade {
public:
    explicit AgentFacade(Broker& b, std::string principal) : broker_(b), principal_(std::move(principal)) { }
    Decision request(std::string permission, std::string resource, std::string justification = {})
    {
        return broker_.request({ principal_, std::move(permission), std::move(resource), std::move(justification) });
    }

private:
    Broker& broker_;
    std::string principal_;
};

// For enforcement points: if the broker could not be reached or answered nothing, the answer is "deny".
inline bool failClosedAllows(std::optional<Decision> d) { return d && d->outcome == Outcome::Allow; }

} // namespace exile::policy
