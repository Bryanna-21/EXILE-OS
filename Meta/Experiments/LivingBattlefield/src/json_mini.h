// Minimal JSON reader, sufficient for assets/sprites.json (objects, numbers, strings).
#pragma once
#include <cctype>
#include <cstdlib>
#include <map>
#include <string>

namespace lb {

struct JV {
    enum Type { Null, Num, Str, Obj } type = Null;
    double num = 0;
    std::string str;
    std::map<std::string, JV> obj;
    JV const* get(std::string const& k) const
    {
        auto it = obj.find(k);
        return it == obj.end() ? nullptr : &it->second;
    }
    double number(std::string const& k, double def = 0) const
    {
        auto* v = get(k);
        return v && v->type == Num ? v->num : def;
    }
};

class JsonParser {
public:
    explicit JsonParser(std::string const& s) : m_s(s) {}
    bool parse(JV& out)
    {
        ws();
        if (!value(out)) return false;
        ws();
        return m_i == m_s.size();
    }

private:
    std::string const& m_s;
    size_t m_i = 0;
    void ws() { while (m_i < m_s.size() && std::isspace((unsigned char)m_s[m_i])) ++m_i; }
    bool str(std::string& o)
    {
        if (m_i >= m_s.size() || m_s[m_i] != '"') return false;
        ++m_i;
        o.clear();
        while (m_i < m_s.size() && m_s[m_i] != '"') {
            if (m_s[m_i] == '\\' && m_i + 1 < m_s.size()) ++m_i;
            o += m_s[m_i++];
        }
        if (m_i >= m_s.size()) return false;
        ++m_i;
        return true;
    }
    bool value(JV& v)
    {
        ws();
        if (m_i >= m_s.size()) return false;
        char c = m_s[m_i];
        if (c == '{') {
            ++m_i; v.type = JV::Obj; ws();
            if (m_i < m_s.size() && m_s[m_i] == '}') { ++m_i; return true; }
            while (true) {
                ws();
                std::string k;
                if (!str(k)) return false;
                ws();
                if (m_i >= m_s.size() || m_s[m_i] != ':') return false;
                ++m_i;
                JV child;
                if (!value(child)) return false;
                v.obj.emplace(std::move(k), std::move(child));
                ws();
                if (m_i < m_s.size() && m_s[m_i] == ',') { ++m_i; continue; }
                if (m_i < m_s.size() && m_s[m_i] == '}') { ++m_i; return true; }
                return false;
            }
        }
        if (c == '"') { v.type = JV::Str; return str(v.str); }
        char* end = nullptr;
        v.num = std::strtod(m_s.c_str() + m_i, &end);
        if (end == m_s.c_str() + m_i) return false;
        v.type = JV::Num;
        m_i = size_t(end - m_s.c_str());
        return true;
    }
};

} // namespace lb
