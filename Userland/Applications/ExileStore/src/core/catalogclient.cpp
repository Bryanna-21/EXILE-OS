#include "core/catalogclient.h"

#include "core/util.h"

namespace fs = std::filesystem;

namespace exile {

bool FileCatalogSource::fetch(std::string& text, std::string& error) {
    auto data = util::readFile(file_, kMaxCatalogBytes, &error);
    if (!data) return false;
    text = std::move(*data);
    return true;
}

CatalogState loadCatalog(ICatalogSource& source, const fs::path& cacheFile) {
    CatalogState state;
    std::string text, fetchError;

    if (source.fetch(text, fetchError)) {
        CatalogLoadResult parsed = parseCatalog(text, source.baseDir());
        if (parsed.ok()) {
            state.catalog = std::move(parsed.catalog);
            state.issues = std::move(parsed.issues);
            std::string werr;
            util::writeFileAtomic(cacheFile, text, werr);  // best effort; failure only costs offline support
            return state;
        }
        fetchError = parsed.issues.empty() ? "invalid catalog" : parsed.issues.front().message;
        state.issues = std::move(parsed.issues);
    }

    // Live catalog unavailable or invalid: fall back to the last good copy.
    std::string cerr;
    auto cached = util::readFile(cacheFile, kMaxCatalogBytes, &cerr);
    if (cached) {
        CatalogLoadResult parsed = parseCatalog(*cached, source.baseDir());
        if (parsed.ok()) {
            state.catalog = std::move(parsed.catalog);
            state.fromCache = true;
            state.notice = "You're viewing saved catalog data" +
                           (state.catalog->generatedAt.empty() ? std::string() : " from " + state.catalog->generatedAt) +
                           " because the latest catalog couldn't be loaded (" + fetchError + ").";
            return state;
        }
    }
    state.notice = "The app catalog could not be loaded (" + fetchError + "), and there is no saved copy to show. "
                   "Your installed apps are still available in Library.";
    return state;
}

}  // namespace exile
