/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Canonical EXILE permission vocabulary, shared by package manifests (XAM), the
// permission broker, and the EIS agent runtime. A permission is "<domain>.<action>[:<scope>]".
// The vocabulary is closed: anything not in the table is rejected, never guessed at.
// Pure data + validation; no I/O, no kernel dependency.

#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace exile::policy {

enum class ScopeKind { None, Path, Host, Exec, Key };
enum class Risk { Low, Medium, High, Critical };

struct PermissionSpec {
    char const* key; // "fs.read"
    ScopeKind scope;
    Risk risk;
    char const* summary;
};

PermissionSpec const* findSpec(std::string_view key);
std::vector<PermissionSpec> const& allSpecs();
char const* toString(Risk);

struct Permission {
    std::string key;   // "fs.read"
    std::string scope; // normalised scope, empty when the spec takes none

    std::string canonical() const; // "fs.read:/home/x" or "ui.notify"
    bool operator==(Permission const&) const = default;
    bool operator<(Permission const& o) const { return key != o.key ? key < o.key : scope < o.scope; }
};

// Strict path normalisation. Accepts absolute ("/a/b") or symbolic-root ("$documents/a") paths.
// Rejects "." / ".." components, empty components, control characters, backslashes, NUL, overlong input.
// It does NOT resolve symlinks: callers must pass real paths; the kernel's unveil is the final authority.
std::optional<std::string> normalizePath(std::string_view in);
// "host:port" with host lowercase DNS name or "*.suffix" or "*"; port 1-65535 or "*".
std::optional<std::string> normalizeHostPort(std::string_view in);

// Parses "key[:scope]" and validates against the vocabulary. Error text is plain language.
std::optional<Permission> parsePermission(std::string_view text, std::string& error);

// True if a grant of `have` allows exactly what `want` asks (same key, scope containment).
bool covers(Permission const& have, Permission const& want);
// True if the scope is deliberately wide (root path, "*" host/port/key, symbolic root alone).
bool isBroad(Permission const& p);
Risk riskOf(Permission const& p);

class PermissionSet {
public:
    static std::optional<PermissionSet> parse(std::vector<std::string> const& items, std::string& error);
    void add(Permission p);
    bool allows(Permission const& want) const;
    std::vector<Permission> const& items() const { return items_; }
    // Permissions in `next` that `*this` does not already cover (the "escalation" of an update).
    std::vector<Permission> escalationTo(PermissionSet const& next) const;
    std::string canonicalText() const; // sorted, one per line
    std::string digestHex() const;     // SHA-256 of canonicalText()

private:
    std::vector<Permission> items_; // sorted, unique
};

// Maps the Store catalog's legacy coarse permission ids ("camera", "documents", ...) to canonical ones.
// Unknown ids are returned in `unmapped` and are NEVER silently granted or dropped.
struct LegacyMapping {
    PermissionSet set;
    std::vector<std::string> unmapped;
};
LegacyMapping mapLegacyIds(std::vector<std::string> const& ids);

// Payload to sign so a signature binds the permission set to the package:
// the Store's v1 payload fields plus the permission digest. Distinct prefix prevents confusion with v1.
std::string signingPayloadV2(std::string_view appId, std::string_view version, std::string_view packageId,
    std::string_view sha256, unsigned long long size, PermissionSet const& declared);

} // namespace exile::policy
