#include <cstring>
#include <iostream>

#include "testing.h"

int main(int argc, char** argv) {
    const char* filter = argc > 1 ? argv[1] : nullptr;
    int passed = 0, failed = 0;
    for (const auto& c : testing::registry()) {
        if (filter && c.name.find(filter) == std::string::npos) continue;
        try {
            c.fn();
            ++passed;
        } catch (const testing::Failure& f) {
            ++failed;
            std::cout << "FAIL " << c.name << "\n     " << f.message << "\n";
        } catch (const std::exception& e) {
            ++failed;
            std::cout << "FAIL " << c.name << "\n     unexpected exception: " << e.what() << "\n";
        }
    }
    std::cout << passed << " passed, " << failed << " failed\n";
#ifndef EXILE_HAS_OPENSSL
    std::cout << "NOTE: built without OpenSSL; signature-dependent tests were not compiled\n";
#endif
    return failed == 0 ? 0 : 1;
}
