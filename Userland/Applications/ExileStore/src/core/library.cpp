/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "library.h"

#include <algorithm>
#include <system_error>

#include "json.h"
#include "util.h"

namespace fs = std::filesystem;

namespace exile {

namespace {
constexpr size_t kMaxHistory = 200;

std::string str(json::Value const& obj, char const* key)
{
    json::Value const* v = obj.find(key);
    return (v && v->string()) ? *v->string() : std::string();
}
} // namespace

bool Library::load(std::string& error)
{
    installed_.clear();
    history_.clear();
    std::error_code ec;
    if (!fs::exists(file_, ec))
        return true;

    auto text = util::readFile(file_, 8 * 1024 * 1024, &error);
    json::Value root;
    bool ok = text.has_value();
    if (ok) {
        try {
            root = json::parse(*text);
            ok = root.isObject();
            if (!ok)
                error = "library file is not a JSON object";
        } catch (json::ParseError const& ex) {
            ok = false;
            error = ex.what();
        }
    }
    if (!ok) {
        fs::path quarantine = file_;
        quarantine += ".corrupt";
        fs::rename(file_, quarantine, ec);
        error = "The saved library could not be read (" + error + "). It was kept as " + quarantine.filename().string() + " and a new one was started.";
        return false;
    }

    if (json::Value const* apps = root.find("installed"); apps && apps->isArray()) {
        for (json::Value const& a : apps->items()) {
            InstalledApp ia;
            ia.appId = str(a, "appId");
            ia.name = str(a, "name");
            ia.version = str(a, "version");
            ia.packageId = str(a, "packageId");
            ia.installedAt = str(a, "installedAt");
            ia.publisherId = str(a, "publisherId");
            ia.keyId = str(a, "keyId");
            ia.sha256 = str(a, "sha256");
            if (!ia.appId.empty() && !ia.version.empty())
                installed_.push_back(std::move(ia));
        }
    }
    if (json::Value const* hist = root.find("history"); hist && hist->isArray()) {
        for (json::Value const& h : hist->items()) {
            HistoryEvent he { str(h, "time"), str(h, "appId"), str(h, "action"), str(h, "version"), str(h, "outcome"), str(h, "detail") };
            if (!he.appId.empty())
                history_.push_back(std::move(he));
        }
    }
    return true;
}

bool Library::save(std::string& error) const
{
    using json::Value;
    Value root = Value::makeObject();
    root.set("schemaVersion", Value::makeNumber(1));
    Value apps = Value::makeArray();
    for (auto const& a : installed_) {
        Value o = Value::makeObject();
        o.set("appId", Value::makeString(a.appId));
        o.set("name", Value::makeString(a.name));
        o.set("version", Value::makeString(a.version));
        o.set("packageId", Value::makeString(a.packageId));
        o.set("installedAt", Value::makeString(a.installedAt));
        o.set("publisherId", Value::makeString(a.publisherId));
        o.set("keyId", Value::makeString(a.keyId));
        o.set("sha256", Value::makeString(a.sha256));
        apps.push(std::move(o));
    }
    root.set("installed", std::move(apps));
    Value hist = Value::makeArray();
    for (auto const& h : history_) {
        Value o = Value::makeObject();
        o.set("time", Value::makeString(h.time));
        o.set("appId", Value::makeString(h.appId));
        o.set("action", Value::makeString(h.action));
        o.set("version", Value::makeString(h.version));
        o.set("outcome", Value::makeString(h.outcome));
        o.set("detail", Value::makeString(h.detail));
        hist.push(std::move(o));
    }
    root.set("history", std::move(hist));
    return util::writeFileAtomic(file_, json::dump(root), error);
}

InstalledApp const* Library::find(std::string const& appId) const
{
    for (auto const& a : installed_)
        if (a.appId == appId)
            return &a;
    return nullptr;
}

void Library::upsert(InstalledApp app)
{
    for (auto& a : installed_) {
        if (a.appId == app.appId) {
            a = std::move(app);
            return;
        }
    }
    installed_.push_back(std::move(app));
}

bool Library::remove(std::string const& appId)
{
    auto it = std::remove_if(installed_.begin(), installed_.end(), [&](InstalledApp const& a) { return a.appId == appId; });
    bool removed = it != installed_.end();
    installed_.erase(it, installed_.end());
    return removed;
}

void Library::addHistory(HistoryEvent e)
{
    history_.push_back(std::move(e));
    if (history_.size() > kMaxHistory)
        history_.erase(history_.begin(), history_.begin() + static_cast<long>(history_.size() - kMaxHistory));
}

} // namespace exile
