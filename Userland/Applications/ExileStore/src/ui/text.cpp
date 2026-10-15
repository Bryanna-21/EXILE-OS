#include "ui/text.h"

#include <algorithm>

namespace exile::ui {

std::string Theme::wrap(const char* code, std::string_view s) const {
    if (!color_) return std::string(s);
    return std::string("\x1b[") + code + "m" + std::string(s) + "\x1b[0m";
}

size_t displayWidth(std::string_view s) {
    size_t w = 0;
    for (size_t i = 0; i < s.size(); ++i) {
        unsigned char c = static_cast<unsigned char>(s[i]);
        if (c == 0x1b && i + 1 < s.size() && s[i + 1] == '[') {
            i += 2;
            while (i < s.size() && !((s[i] >= '@' && s[i] <= '~'))) ++i;
            continue;
        }
        if ((c & 0xC0) != 0x80) ++w;
    }
    return w;
}

std::string truncate(std::string_view s, size_t width) {
    if (displayWidth(s) <= width) return std::string(s);
    if (width <= 3) return std::string(width, '.');
    size_t keep = width - 3;
    size_t cols = 0, i = 0;
    while (i < s.size()) {
        unsigned char c = static_cast<unsigned char>(s[i]);
        if ((c & 0xC0) != 0x80) {
            if (cols == keep) break;
            ++cols;
        }
        ++i;
    }
    return std::string(s.substr(0, i)) + "...";
}

std::string padRight(std::string_view s, size_t width) {
    size_t w = displayWidth(s);
    std::string out(s);
    if (w < width) out.append(width - w, ' ');
    return out;
}

std::string repeat(char c, size_t n) { return std::string(n, c); }

std::vector<std::string> wrapText(std::string_view text, size_t width) {
    std::vector<std::string> lines;
    if (width < 8) width = 8;
    size_t start = 0;
    while (start <= text.size()) {
        size_t nl = text.find('\n', start);
        std::string_view para = text.substr(start, nl == std::string_view::npos ? std::string_view::npos : nl - start);
        std::string line;
        size_t pos = 0;
        bool any = false;
        while (pos < para.size()) {
            while (pos < para.size() && para[pos] == ' ') ++pos;
            size_t end = para.find(' ', pos);
            if (end == std::string_view::npos) end = para.size();
            std::string word(para.substr(pos, end - pos));
            pos = end;
            if (word.empty()) continue;
            any = true;
            if (displayWidth(word) > width) word = truncate(word, width);
            if (line.empty()) line = word;
            else if (displayWidth(line) + 1 + displayWidth(word) <= width) line += " " + word;
            else { lines.push_back(line); line = word; }
        }
        if (!line.empty()) lines.push_back(line);
        else if (!any) lines.push_back("");
        if (nl == std::string_view::npos) break;
        start = nl + 1;
    }
    while (!lines.empty() && lines.back().empty()) lines.pop_back();
    return lines;
}

std::string progressBar(double fraction, size_t width) {
    if (width < 4) width = 4;
    fraction = std::clamp(fraction, 0.0, 1.0);
    size_t inner = width - 2;
    size_t filled = static_cast<size_t>(fraction * static_cast<double>(inner) + 0.5);
    return "[" + std::string(filled, '#') + std::string(inner - filled, '-') + "]";
}

}  // namespace exile::ui
