# The boot guardian and the Ed25519/SHA-2 code it is verified against are compiled directly into the tool
# instead of linking LibCrypto: host tools are also built without BUILD_LAGOM (no LibCrypto/LibCore target),
# and this keeps the tool using the same sources as the prekernel verifier.
lagom_tool(ExileBootManifest
    SOURCES
        ${ExileOS_SOURCE_DIR}/Meta/Tools/ExileBootManifest.cpp
        ${ExileOS_SOURCE_DIR}/Kernel/Boot/Exile/BootGuardian.cpp
        ${ExileOS_SOURCE_DIR}/Userland/Libraries/LibCrypto/Curves/Curve25519.cpp
        ${ExileOS_SOURCE_DIR}/Userland/Libraries/LibCrypto/Curves/Ed25519.cpp
        ${ExileOS_SOURCE_DIR}/Userland/Libraries/LibCrypto/Hash/SHA2.cpp
)
