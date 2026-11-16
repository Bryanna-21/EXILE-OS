/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "permission.h"

#include <algorithm>
#include <cctype>

#include "sha256.h" // reused from ExileStore (src/core)

namespace exile::policy {

namespace {
const PermissionSpec kSpecs[] = {
    { "fs.read", ScopeKind::Path, Risk::Medium, "Read files under a path" },
    { "fs.write", ScopeKind::Path, Risk::High, "Create, change or delete files under a path" },
    { "net.connect", ScopeKind::Host, Risk::Medium, "Open outgoing network connections to a host:port" },
    { "proc.spawn", ScopeKind::Exec, Risk::High, "Start a specific program" },
    { "proc.background", ScopeKind::None, Risk::Medium, "Keep running when not in use" },
    { "sys.settings", ScopeKind::Key, Risk::Critical, "Change system settings" },
    { "sys.install", ScopeKind::None, Risk::Critical, "Install, update or remove applications" },
    { "dev.camera", ScopeKind::None, Risk::High, "Use the camera" },
    { "dev.microphone", ScopeKind::None, Risk::High, "Use the microphone" },
    { "dev.location", ScopeKind::None, Risk::High, "See where this device is" },
    { "dev.bluetooth", ScopeKind::None, Risk::Medium, "Use Bluetooth" },
    { "dev.usb", ScopeKind::None, Risk::High, "Talk to USB devices" },
    { "data.contacts", ScopeKind::None, Risk::High, "Read contacts" },
    { "ui.notify", ScopeKind::None, Risk::Low, "Show notifications" },
    { "ui.clipboard", ScopeKind::None, Risk::Medium, "Read the clipboard" },
    { "ai.cloud", ScopeKind::Host, Risk::High, "Send data to a cloud AI provider host:port" },
};

bool badChar(unsigned char c) { return c < 0x20 || c == 0x7f || c == '\\'; }

bool validDnsLabel(std::string_view l)
{
    if (l.empty() || l.size() > 63 || l.front() == '-' || l.back() == '-')
        return false;
    return std::all_of(l.begin(), l.end(), [](unsigned char c) { return std::isalnum(c) || c == '-'; });
}
} // namespace

std::vector<PermissionSpec> const& allSpecs()
{
    static std::vector<PermissionSpec> v(std::begin(kSpecs), std::end(kSpecs));
    return v;
}

PermissionSpec const* findSpec(std::string_view key)
{
    for (auto const& s : kSpecs)
        if (key == s.key)
            return &s;
    return nullptr;
}

char const* toString(Risk r)
{
    switch (r) {
    case Risk::Low: return "low";
    case Risk::Medium: return "medium";
    case Risk::High: return "high";
    case Risk::Critical: return "critical";
    }
    return "?";
}

std::string Permission::canonical() const { return scope.empty() ? key : key + ":" + scope; }

std::optional<std::string> normalizePath(std::string_view in)
{
    if (in.empty() || in.size() > 4096 || (in[0] != '/' && in[0] != '$'))
        return std::nullopt;
    for (char c : in)
        if (badChar(static_cast<unsigned char>(c)))
            return std::nullopt;
    if (in == "/")
        return std::string("/");
    std::string out;
    size_t i = 0;
    bool first = true;
    while (i < in.size()) {
        if (in[i] == '/')
            ++i;
        size_t j = in.find('/', i);
        std::string_view comp = in.substr(i, j == std::string_view::npos ? std::string_view::npos : j - i);
        if (j == std::string_view::npos && comp.empty() && !first)
            break; // single trailing slash is dropped
        if (comp.empty() || comp == "." || comp == ".." || comp.size() > 255)
            return std::nullopt;
        if (first && in[0] == '$') {
            if (comp.size() < 2)
                return std::nullopt;
            for (size_t k = 1; k < comp.size(); ++k)
                if (!(std::islower(static_cast<unsigned char>(comp[k])) || std::isdigit(static_cast<unsigned char>(comp[k])) || comp[k] == '-'))
                    return std::nullopt;
            out = std::string(comp);
        } else {
            out += "/" + std::string(comp);
        }
        first = false;
        if (j == std::string_view::npos)
            break;
        i = j;
    }
    return out;
}

std::optional<std::string> normalizeHostPort(std::string_view in)
{
    size_t colon = in.rfind(':');
    if (colon == std::string_view::npos || colon == 0 || in.size() > 300)
        return std::nullopt;
    std::string host(in.substr(0, colon));
    std::string_view port = in.substr(colon + 1);
    for (auto& c : host)
        c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
    if (port != "*") {
        if (port.empty() || port.size() > 5 || !std::all_of(port.begin(), port.end(), [](unsigned char c) { return std::isdigit(c); }))
            return std::nullopt;
        int p = std::stoi(std::string(port));
        if (p < 1 || p > 65535)
            return std::nullopt;
        if (port.size() > 1 && port[0] == '0')
            return std::nullopt;
    }
    if (host != "*") {
        std::string_view h = host;
        if (h.substr(0, 2) == "*.")
            h.remove_prefix(2);
        if (h.empty() || h.size() > 253)
            return std::nullopt;
        size_t start = 0;
        while (start <= h.size()) {
            size_t dot = h.find('.', start);
            auto label = h.substr(start, dot == std::string_view::npos ? std::string_view::npos : dot - start);
            if (!validDnsLabel(label))
                return std::nullopt;
            if (dot == std::string_view::npos)
                break;
            start = dot + 1;
        }
    }
    return host + ":" + std::string(port);
}

std::optional<Permission> parsePermission(std::string_view text, std::string& error)
{
    size_t colon = text.find(':');
    std::string key(text.substr(0, colon));
    PermissionSpec const* spec = findSpec(key);
    if (!spec) {
        error = "unknown permission \"" + key.substr(0, 64) + "\"";
        return std::nullopt;
    }
    Permission p;
    p.key = key;
    bool hasScope = colon != std::string_view::npos;
    std::string_view raw = hasScope ? text.substr(colon + 1) : std::string_view {};
    switch (spec->scope) {
    case ScopeKind::None:
        if (hasScope) {
            error = key + " takes no scope";
            return std::nullopt;
        }
        break;
    case ScopeKind::Path:
    case ScopeKind::Exec: {
        auto n = hasScope ? normalizePath(raw) : std::nullopt;
        if (!n || (spec->scope == ScopeKind::Exec && (*n == "/" || (*n)[0] == '$'))) {
            error = key + " needs a valid absolute path scope" + (spec->scope == ScopeKind::Exec ? " naming one program" : "");
            return std::nullopt;
        }
        p.scope = *n;
        break;
    }
    case ScopeKind::Host: {
        auto n = hasScope ? normalizeHostPort(raw) : std::nullopt;
        if (!n) {
            error = key + " needs a host:port scope";
            return std::nullopt;
        }
        p.scope = *n;
        break;
    }
    case ScopeKind::Key: {
        bool ok = hasScope && !raw.empty() && raw.size() <= 128
            && std::all_of(raw.begin(), raw.end(), [](unsigned char c) { return std::isalnum(c) || c == '.' || c == '-' || c == '_' || c == '*'; });
        if (!ok) {
            error = key + " needs a settings-key scope";
            return std::nullopt;
        }
        p.scope = std::string(raw);
        break;
    }
    }
    return p;
}

namespace {
bool pathCovers(std::string const& have, std::string const& want)
{
    if (have == want || have == "/")
        return true;
    return want.size() > have.size() && want.compare(0, have.size(), have) == 0 && want[have.size()] == '/';
}

bool hostCovers(std::string const& have, std::string const& want)
{
    auto split = [](std::string const& s) { size_t c = s.rfind(':'); return std::pair { s.substr(0, c), s.substr(c + 1) }; };
    auto [hh, hp] = split(have);
    auto [wh, wp] = split(want);
    if (hp != "*" && hp != wp)
        return false;
    if (hh == "*" || hh == wh)
        return true;
    if (hh.substr(0, 2) == "*.") {
        std::string suffix = hh.substr(1); // ".example.com"
        if (wh.substr(0, 2) == "*.")
            wh = wh.substr(1), suffix = hh.substr(1);
        return wh.size() > suffix.size() && wh.compare(wh.size() - suffix.size(), suffix.size(), suffix) == 0;
    }
    return false;
}
} // namespace

bool covers(Permission const& have, Permission const& want)
{
    if (have.key != want.key)
        return false;
    PermissionSpec const* spec = findSpec(have.key);
    if (!spec)
        return false;
    switch (spec->scope) {
    case ScopeKind::None: return true;
    case ScopeKind::Path: return pathCovers(have.scope, want.scope);
    case ScopeKind::Exec: return have.scope == want.scope;
    case ScopeKind::Host: return hostCovers(have.scope, want.scope);
    case ScopeKind::Key:
        return have.scope == want.scope || have.scope == "*";
    }
    return false;
}

bool isBroad(Permission const& p)
{
    PermissionSpec const* spec = findSpec(p.key);
    if (!spec)
        return true;
    switch (spec->scope) {
    case ScopeKind::Path: return p.scope == "/" || p.scope.find('/') == std::string::npos; // "/" or bare "$root"
    case ScopeKind::Host: {
        size_t c = p.scope.rfind(':');
        std::string host = p.scope.substr(0, c), port = p.scope.substr(c + 1);
        return host == "*" || host.substr(0, 2) == "*." || port == "*";
    }
    case ScopeKind::Key: return p.scope == "*";
    default: return false;
    }
}

Risk riskOf(Permission const& p)
{
    auto* s = findSpec(p.key);
    return s ? s->risk : Risk::Critical;
}

std::optional<PermissionSet> PermissionSet::parse(std::vector<std::string> const& items, std::string& error)
{
    PermissionSet set;
    if (items.size() > 256) {
        error = "too many permissions (max 256)";
        return std::nullopt;
    }
    for (auto const& i : items) {
        auto p = parsePermission(i, error);
        if (!p)
            return std::nullopt;
        set.add(*p);
    }
    return set;
}

void PermissionSet::add(Permission p)
{
    auto it = std::lower_bound(items_.begin(), items_.end(), p);
    if (it == items_.end() || !(*it == p))
        items_.insert(it, std::move(p));
}

bool PermissionSet::allows(Permission const& want) const
{
    return std::any_of(items_.begin(), items_.end(), [&](Permission const& h) { return covers(h, want); });
}

std::vector<Permission> PermissionSet::escalationTo(PermissionSet const& next) const
{
    std::vector<Permission> out;
    for (auto const& p : next.items_)
        if (!allows(p))
            out.push_back(p);
    return out;
}

std::string PermissionSet::canonicalText() const
{
    std::string s;
    for (auto const& p : items_)
        s += p.canonical() + "\n";
    return s;
}

std::string PermissionSet::digestHex() const { return exile::sha256Hex(canonicalText()); }

LegacyMapping mapLegacyIds(std::vector<std::string> const& ids)
{
    static const struct {
        char const* id;
        std::vector<char const*> perms;
    } table[] = {
        { "network", { "net.connect:*:*" } },
        { "camera", { "dev.camera" } },
        { "microphone", { "dev.microphone" } },
        { "location", { "dev.location" } },
        { "documents", { "fs.read:$documents", "fs.write:$documents" } },
        { "photos", { "fs.read:$photos", "fs.write:$photos" } },
        { "contacts", { "data.contacts" } },
        { "notifications", { "ui.notify" } },
        { "background", { "proc.background" } },
        { "bluetooth", { "dev.bluetooth" } },
        { "usb", { "dev.usb" } },
        { "clipboard", { "ui.clipboard" } },
        { "system-settings", { "sys.settings:*" } },
    };
    LegacyMapping m;
    for (auto const& id : ids) {
        bool found = false;
        for (auto const& e : table) {
            if (id != e.id)
                continue;
            found = true;
            for (char const* p : e.perms) {
                std::string err;
                auto perm = parsePermission(p, err);
                if (perm)
                    m.set.add(*perm);
                else
                    m.unmapped.push_back(id); // table bug; surfaced, never ignored
            }
        }
        if (!found)
            m.unmapped.push_back(id);
    }
    return m;
}

std::string signingPayloadV2(std::string_view appId, std::string_view version, std::string_view packageId,
    std::string_view sha256, unsigned long long size, PermissionSet const& declared)
{
    return "exile-package-signature-v2\n" + std::string(appId) + "\n" + std::string(version) + "\n" + std::string(packageId)
        + "\n" + std::string(sha256) + "\n" + std::to_string(size) + "\npermissions-sha256:" + declared.digestHex() + "\n";
}

} // namespace exile::policy
