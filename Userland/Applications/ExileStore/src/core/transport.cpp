#include "core/transport.h"

#include <fstream>
#include <system_error>

#include "core/catalog.h"

namespace fs = std::filesystem;

namespace exile {

namespace {
bool isUnder(const fs::path& child, const fs::path& parent) {
    auto c = child.begin();
    for (auto p = parent.begin(); p != parent.end(); ++p, ++c) {
        if (c == child.end() || *c != *p) return false;
    }
    return true;
}
}  // namespace

std::optional<fs::path> resolveLocalSource(const std::string& source, const fs::path& baseDir, bool allowFileUrls,
                                           std::string& error) {
    std::string why;
    if (!isAcceptableSourceUrl(source, &why)) {
        error = "refusing package source: " + why;
        return std::nullopt;
    }
    if (source.rfind("https://", 0) == 0) {
        error = "not a local source";
        return std::nullopt;
    }
    std::error_code ec;
    if (source.rfind("file://", 0) == 0) {
        if (!allowFileUrls) {
            error = "absolute file:// package sources are disabled";
            return std::nullopt;
        }
        return fs::path(source.substr(7));
    }
    fs::path base = fs::weakly_canonical(baseDir, ec);
    if (ec) { error = "cannot resolve catalog directory"; return std::nullopt; }
    fs::path full = fs::weakly_canonical(base / source, ec);
    if (ec) { error = "cannot resolve package path"; return std::nullopt; }
    if (!isUnder(full, base)) {
        error = "package path escapes the catalog directory";
        return std::nullopt;
    }
    return full;
}

TransferResult LocalTransport::fetch(const std::string& source, const fs::path& partFile, bool resume,
                                     const ProgressFn& progress, const CancelToken& cancel) {
    TransferResult r;
    auto src = resolveLocalSource(source, baseDir_, allowFileUrls_, r.error);
    if (!src) return r;

    std::error_code ec;
    if (!fs::is_regular_file(*src, ec)) {
        r.error = "package file not found: " + src->filename().string();
        return r;
    }
    uint64_t total = fs::file_size(*src, ec);
    if (ec) { r.error = "cannot read package size"; return r; }

    uint64_t offset = 0;
    if (resume && fs::exists(partFile, ec)) {
        offset = fs::file_size(partFile, ec);
        if (ec || offset > total) offset = 0;  // stale or oversized partial: start over
    }
    if (partFile.has_parent_path()) fs::create_directories(partFile.parent_path(), ec);

    std::ifstream in(*src, std::ios::binary);
    if (!in) { r.error = "cannot open package file"; return r; }
    in.seekg(static_cast<std::streamoff>(offset));
    std::ofstream out(partFile, std::ios::binary | (offset ? std::ios::app : std::ios::trunc));
    if (!out) { r.error = "cannot write to download cache"; return r; }

    char buf[64 * 1024];
    uint64_t done = offset;
    if (progress) progress(done, total);
    while (done < total) {
        if (cancel.cancelled()) {
            out.flush();
            r.cancelled = true;
            r.error = "cancelled";
            r.bytes = done;
            return r;
        }
        in.read(buf, sizeof buf);
        std::streamsize got = in.gcount();
        if (got <= 0) break;
        out.write(buf, got);
        if (!out) { r.error = "write failed (disk full?)"; return r; }
        done += static_cast<uint64_t>(got);
        if (progress) progress(done, total);
    }
    out.flush();
    if (done != total) { r.error = "source ended early"; return r; }
    r.ok = true;
    r.bytes = done;
    return r;
}

TransferResult UnavailableHttpsTransport::fetch(const std::string&, const fs::path&, bool, const ProgressFn&, const CancelToken&) {
    TransferResult r;
    r.error = "downloading over https is not available in this build (no TLS transport is linked)";
    return r;
}

TransferResult RoutingTransport::fetch(const std::string& source, const fs::path& partFile, bool resume,
                                       const ProgressFn& progress, const CancelToken& cancel) {
    if (source.rfind("https://", 0) == 0) return remote_.fetch(source, partFile, resume, progress, cancel);
    return local_.fetch(source, partFile, resume, progress, cancel);
}

}  // namespace exile
