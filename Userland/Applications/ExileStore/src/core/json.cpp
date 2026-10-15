#include "core/json.h"

#include <cmath>
#include <cstdio>
#include <cstdlib>

namespace exile::json {

Value Value::makeBool(bool b) { Value v; v.type_ = Type::Bool; v.bool_ = b; return v; }
Value Value::makeNumber(double n) { Value v; v.type_ = Type::Number; v.num_ = n; return v; }
Value Value::makeString(std::string s) { Value v; v.type_ = Type::String; v.str_ = std::move(s); return v; }
Value Value::makeArray() { Value v; v.type_ = Type::Array; return v; }
Value Value::makeObject() { Value v; v.type_ = Type::Object; return v; }

std::optional<double> Value::number() const {
    if (type_ != Type::Number) return std::nullopt;
    return num_;
}
std::optional<bool> Value::boolean() const {
    if (type_ != Type::Bool) return std::nullopt;
    return bool_;
}
std::optional<uint64_t> Value::unsignedInteger() const {
    if (type_ != Type::Number) return std::nullopt;
    if (!(num_ >= 0) || num_ > 9007199254740992.0 || std::floor(num_) != num_) return std::nullopt;
    return static_cast<uint64_t>(num_);
}

const Value* Value::find(std::string_view key) const {
    if (type_ != Type::Object) return nullptr;
    for (size_t i = 0; i < keys_.size(); ++i)
        if (keys_[i] == key) return &items_[i];
    return nullptr;
}

void Value::push(Value v) { items_.push_back(std::move(v)); }
void Value::set(std::string key, Value v) {
    for (size_t i = 0; i < keys_.size(); ++i) {
        if (keys_[i] == key) { items_[i] = std::move(v); return; }
    }
    keys_.push_back(std::move(key));
    items_.push_back(std::move(v));
}

namespace {

constexpr int kMaxDepth = 64;

class Parser {
public:
    explicit Parser(std::string_view s) : s_(s) {}

    Value document() {
        skipWs();
        Value v = value(0);
        skipWs();
        if (pos_ != s_.size()) fail("unexpected trailing characters");
        return v;
    }

private:
    [[noreturn]] void fail(const std::string& msg) const {
        throw ParseError("JSON error at offset " + std::to_string(pos_) + ": " + msg);
    }
    bool eof() const { return pos_ >= s_.size(); }
    char peek() const { return eof() ? '\0' : s_[pos_]; }
    void skipWs() {
        while (!eof() && (s_[pos_] == ' ' || s_[pos_] == '\t' || s_[pos_] == '\n' || s_[pos_] == '\r')) ++pos_;
    }
    void expect(char c) {
        if (peek() != c) fail(std::string("expected '") + c + "'");
        ++pos_;
    }
    bool literal(std::string_view lit) {
        if (s_.substr(pos_, lit.size()) == lit) { pos_ += lit.size(); return true; }
        return false;
    }

    Value value(int depth) {
        if (depth > kMaxDepth) fail("nesting too deep");
        if (eof()) fail("unexpected end of input");
        char c = peek();
        if (c == '{') return object(depth);
        if (c == '[') return array(depth);
        if (c == '"') return Value::makeString(string());
        if (c == 't') { if (literal("true")) return Value::makeBool(true); fail("invalid literal"); }
        if (c == 'f') { if (literal("false")) return Value::makeBool(false); fail("invalid literal"); }
        if (c == 'n') { if (literal("null")) return Value(); fail("invalid literal"); }
        if (c == '-' || (c >= '0' && c <= '9')) return number();
        fail("unexpected character");
    }

    Value object(int depth) {
        expect('{');
        Value obj = Value::makeObject();
        skipWs();
        if (peek() == '}') { ++pos_; return obj; }
        while (true) {
            skipWs();
            if (peek() != '"') fail("expected string key");
            std::string key = string();
            if (obj.find(key)) fail("duplicate key '" + key + "'");
            skipWs();
            expect(':');
            skipWs();
            obj.set(std::move(key), value(depth + 1));
            skipWs();
            if (peek() == ',') { ++pos_; continue; }
            expect('}');
            return obj;
        }
    }

    Value array(int depth) {
        expect('[');
        Value arr = Value::makeArray();
        skipWs();
        if (peek() == ']') { ++pos_; return arr; }
        while (true) {
            skipWs();
            arr.push(value(depth + 1));
            skipWs();
            if (peek() == ',') { ++pos_; continue; }
            expect(']');
            return arr;
        }
    }

    unsigned hex4() {
        if (pos_ + 4 > s_.size()) fail("truncated \\u escape");
        unsigned v = 0;
        for (int i = 0; i < 4; ++i) {
            char c = s_[pos_++];
            v <<= 4;
            if (c >= '0' && c <= '9') v |= static_cast<unsigned>(c - '0');
            else if (c >= 'a' && c <= 'f') v |= static_cast<unsigned>(c - 'a' + 10);
            else if (c >= 'A' && c <= 'F') v |= static_cast<unsigned>(c - 'A' + 10);
            else fail("bad hex digit in \\u escape");
        }
        return v;
    }

    static void appendUtf8(std::string& out, unsigned cp) {
        if (cp < 0x80) out.push_back(static_cast<char>(cp));
        else if (cp < 0x800) {
            out.push_back(static_cast<char>(0xC0 | (cp >> 6)));
            out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
        } else if (cp < 0x10000) {
            out.push_back(static_cast<char>(0xE0 | (cp >> 12)));
            out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F)));
            out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
        } else {
            out.push_back(static_cast<char>(0xF0 | (cp >> 18)));
            out.push_back(static_cast<char>(0x80 | ((cp >> 12) & 0x3F)));
            out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F)));
            out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
        }
    }

    std::string string() {
        expect('"');
        std::string out;
        while (true) {
            if (eof()) fail("unterminated string");
            unsigned char c = static_cast<unsigned char>(s_[pos_++]);
            if (c == '"') return out;
            if (c < 0x20) fail("control character in string");
            if (c != '\\') { out.push_back(static_cast<char>(c)); continue; }
            if (eof()) fail("unterminated escape");
            char e = s_[pos_++];
            switch (e) {
                case '"': out.push_back('"'); break;
                case '\\': out.push_back('\\'); break;
                case '/': out.push_back('/'); break;
                case 'b': out.push_back('\b'); break;
                case 'f': out.push_back('\f'); break;
                case 'n': out.push_back('\n'); break;
                case 'r': out.push_back('\r'); break;
                case 't': out.push_back('\t'); break;
                case 'u': {
                    unsigned cp = hex4();
                    if (cp >= 0xD800 && cp <= 0xDBFF) {
                        if (!(literal("\\u"))) fail("lone high surrogate");
                        unsigned lo = hex4();
                        if (lo < 0xDC00 || lo > 0xDFFF) fail("invalid surrogate pair");
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    } else if (cp >= 0xDC00 && cp <= 0xDFFF) {
                        fail("lone low surrogate");
                    }
                    appendUtf8(out, cp);
                    break;
                }
                default: fail("invalid escape");
            }
        }
    }

    Value number() {
        size_t start = pos_;
        if (peek() == '-') ++pos_;
        if (peek() == '0') {
            ++pos_;
        } else if (peek() >= '1' && peek() <= '9') {
            while (peek() >= '0' && peek() <= '9') ++pos_;
        } else {
            fail("invalid number");
        }
        if (peek() == '.') {
            ++pos_;
            if (!(peek() >= '0' && peek() <= '9')) fail("invalid number");
            while (peek() >= '0' && peek() <= '9') ++pos_;
        }
        if (peek() == 'e' || peek() == 'E') {
            ++pos_;
            if (peek() == '+' || peek() == '-') ++pos_;
            if (!(peek() >= '0' && peek() <= '9')) fail("invalid number");
            while (peek() >= '0' && peek() <= '9') ++pos_;
        }
        std::string text(s_.substr(start, pos_ - start));
        double d = std::strtod(text.c_str(), nullptr);
        if (!std::isfinite(d)) fail("number out of range");
        return Value::makeNumber(d);
    }

    std::string_view s_;
    size_t pos_ = 0;
};

void escapeInto(std::string& out, const std::string& s) {
    out.push_back('"');
    for (unsigned char c : s) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    std::snprintf(buf, sizeof buf, "\\u%04x", c);
                    out += buf;
                } else {
                    out.push_back(static_cast<char>(c));
                }
        }
    }
    out.push_back('"');
}

void dumpInto(std::string& out, const Value& v, int indent, int level) {
    auto nl = [&](int lvl) {
        if (indent <= 0) return;
        out.push_back('\n');
        out.append(static_cast<size_t>(indent * lvl), ' ');
    };
    switch (v.type()) {
        case Value::Type::Null: out += "null"; break;
        case Value::Type::Bool: out += *v.boolean() ? "true" : "false"; break;
        case Value::Type::Number: {
            double d = *v.number();
            char buf[40];
            if (std::floor(d) == d && std::fabs(d) < 1e15) std::snprintf(buf, sizeof buf, "%.0f", d);
            else std::snprintf(buf, sizeof buf, "%.17g", d);
            out += buf;
            break;
        }
        case Value::Type::String: escapeInto(out, *v.string()); break;
        case Value::Type::Array:
            if (v.items().empty()) { out += "[]"; break; }
            out.push_back('[');
            for (size_t i = 0; i < v.items().size(); ++i) {
                nl(level + 1);
                dumpInto(out, v.items()[i], indent, level + 1);
                if (i + 1 < v.items().size()) out.push_back(',');
            }
            nl(level);
            out.push_back(']');
            break;
        case Value::Type::Object:
            if (v.keys().empty()) { out += "{}"; break; }
            out.push_back('{');
            for (size_t i = 0; i < v.keys().size(); ++i) {
                nl(level + 1);
                escapeInto(out, v.keys()[i]);
                out += indent > 0 ? ": " : ":";
                dumpInto(out, v.items()[i], indent, level + 1);
                if (i + 1 < v.keys().size()) out.push_back(',');
            }
            nl(level);
            out.push_back('}');
            break;
    }
}

}  // namespace

Value parse(std::string_view text) { return Parser(text).document(); }

std::string dump(const Value& v, int indent) {
    std::string out;
    dumpInto(out, v, indent, 0);
    return out;
}

}  // namespace exile::json
