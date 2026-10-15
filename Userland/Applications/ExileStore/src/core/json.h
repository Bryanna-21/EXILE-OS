#pragma once
// Minimal strict JSON reader/writer. No external dependencies.
// Strictness is intentional: duplicate object keys, trailing garbage, lone
// surrogates and excessive nesting are all rejected, because catalog metadata
// is untrusted input.

#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace exile::json {

class ParseError : public std::runtime_error {
public:
    using std::runtime_error::runtime_error;
};

class Value {
public:
    enum class Type { Null, Bool, Number, String, Array, Object };

    Value() = default;
    static Value makeBool(bool b);
    static Value makeNumber(double n);
    static Value makeString(std::string s);
    static Value makeArray();
    static Value makeObject();

    Type type() const { return type_; }
    bool isNull() const { return type_ == Type::Null; }
    bool isObject() const { return type_ == Type::Object; }
    bool isArray() const { return type_ == Type::Array; }
    bool isString() const { return type_ == Type::String; }
    bool isNumber() const { return type_ == Type::Number; }
    bool isBool() const { return type_ == Type::Bool; }

    // Non-throwing accessors: null / nullopt when the type does not match.
    const std::string* string() const { return type_ == Type::String ? &str_ : nullptr; }
    std::optional<double> number() const;
    std::optional<bool> boolean() const;
    // Non-negative integer that fits in 2^53, otherwise nullopt.
    std::optional<uint64_t> unsignedInteger() const;

    // Object lookup (nullptr when not an object or key absent).
    const Value* find(std::string_view key) const;
    // Array elements, or object values (parallel to keys()).
    const std::vector<Value>& items() const { return items_; }
    const std::vector<std::string>& keys() const { return keys_; }

    // Builders (used by the serializer side).
    void push(Value v);
    void set(std::string key, Value v);

private:
    Type type_ = Type::Null;
    bool bool_ = false;
    double num_ = 0;
    std::string str_;
    std::vector<Value> items_;
    std::vector<std::string> keys_;
};

Value parse(std::string_view text);  // throws ParseError
std::string dump(const Value& v, int indent = 2);

}  // namespace exile::json
