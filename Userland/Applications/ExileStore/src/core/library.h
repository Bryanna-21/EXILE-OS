#pragma once
// Local record of what the store has installed, plus a bounded install history.
// Persisted as JSON with atomic replace. A corrupt file is quarantined (renamed)
// rather than overwritten, so nothing is silently destroyed.

#include <filesystem>
#include <string>
#include <vector>

namespace exile {

struct InstalledApp {
    std::string appId;
    std::string name;
    std::string version;
    std::string packageId;
    std::string installedAt;
    std::string publisherId;
    std::string keyId;
    std::string sha256;
};

struct HistoryEvent {
    std::string time;
    std::string appId;
    std::string action;   // "install" | "update" | "uninstall"
    std::string version;
    std::string outcome;  // "success" | "failed" | "cancelled"
    std::string detail;
};

class Library {
public:
    explicit Library(std::filesystem::path file) : file_(std::move(file)) {}

    // True when loaded or when no file exists yet. False when the file was
    // unreadable/corrupt: state is then empty and `error` explains what happened.
    bool load(std::string& error);
    bool save(std::string& error) const;

    const std::vector<InstalledApp>& installed() const { return installed_; }
    const std::vector<HistoryEvent>& history() const { return history_; }
    const InstalledApp* find(const std::string& appId) const;

    void upsert(InstalledApp app);
    bool remove(const std::string& appId);
    void addHistory(HistoryEvent e);

private:
    std::filesystem::path file_;
    std::vector<InstalledApp> installed_;
    std::vector<HistoryEvent> history_;
};

}  // namespace exile
