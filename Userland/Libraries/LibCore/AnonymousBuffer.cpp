/*
 * Copyright (c) 2021, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2024, the SerenityOS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/Try.h>
#include <LibCore/AnonymousBuffer.h>
#include <LibCore/System.h>
#include <LibIPC/File.h>
#include <fcntl.h>
#if !defined(AK_OS_WINDOWS)
#    include <sys/mman.h>
#endif

#if defined(AK_OS_WINDOWS)
static constexpr int ExileProtRead = 0x1;
static constexpr int ExileProtWrite = 0x2;
static constexpr int ExileMapShared = 0x1;
#else
#    define ExileProtRead PROT_READ
#    define ExileProtWrite PROT_WRITE
#    define ExileMapShared MAP_SHARED
#endif

namespace Core {

ErrorOr<AnonymousBuffer> AnonymousBuffer::create_with_size(size_t size)
{
#if defined(AK_OS_WINDOWS)
    auto fd = TRY(Core::System::anon_create(size, 0));
#else
    auto fd = TRY(Core::System::anon_create(size, O_CLOEXEC));
#endif
    return create_from_anon_fd(fd, size);
}

ErrorOr<NonnullRefPtr<AnonymousBufferImpl>> AnonymousBufferImpl::create(int fd, size_t size)
{
    auto data = Core::System::mmap(nullptr, round_up_to_power_of_two(size, PAGE_SIZE), ExileProtRead | ExileProtWrite, ExileMapShared, fd, 0);
    if (data.is_error()) {
        auto saved_error = data.release_error();
        (void)Core::System::close(fd);
        return saved_error;
    }
    return AK::adopt_nonnull_ref_or_enomem(new (nothrow) AnonymousBufferImpl(fd, size, data.release_value()));
}

AnonymousBufferImpl::~AnonymousBufferImpl()
{
    if (m_fd != -1) {
        auto rc = Core::System::close(m_fd);
        VERIFY(!rc.is_error());
    }
    auto rc = Core::System::munmap(m_data, round_up_to_power_of_two(m_size, PAGE_SIZE));
    VERIFY(!rc.is_error());
}

ErrorOr<AnonymousBuffer> AnonymousBuffer::create_from_anon_fd(int fd, size_t size)
{
    auto impl = TRY(AnonymousBufferImpl::create(fd, size));
    return AnonymousBuffer(move(impl));
}

AnonymousBuffer::AnonymousBuffer(NonnullRefPtr<AnonymousBufferImpl> impl)
    : m_impl(move(impl))
{
}

AnonymousBufferImpl::AnonymousBufferImpl(int fd, size_t size, void* data)
    : m_fd(fd)
    , m_size(size)
    , m_data(data)
{
}

}
