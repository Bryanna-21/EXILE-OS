/*
 * Copyright (c) 2020, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2022, Alexander Narsudinov <a.narsudinov@gmail.com>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/IPv4Address.h>
#include <AK/Types.h>
#include <LibCore/Notifier.h>
#include <LibCore/System.h>
#include <LibCore/UDPServer.h>
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

#ifndef SOCK_NONBLOCK
#    if !defined(AK_OS_WINDOWS)
#        include <fcntl.h>
#        include <sys/ioctl.h>
#    endif
#endif

namespace Core {

UDPServer::UDPServer(EventReceiver* parent)
    : EventReceiver(parent)
{
#if defined(AK_OS_WINDOWS)
    auto socket = Core::System::socket(AF_INET, SOCK_DGRAM, 0);
    VERIFY(!socket.is_error());
    m_fd = socket.release_value();

    unsigned long mode = 1;
    VERIFY(!Core::System::ioctl(m_fd, FIONBIO, &mode).is_error());
#elif defined(SOCK_NONBLOCK)
    m_fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
#else
    m_fd = socket(AF_INET, SOCK_DGRAM, 0);
    int option = 1;
    ioctl(m_fd, FIONBIO, &option);
    fcntl(m_fd, F_SETFD, FD_CLOEXEC);
#endif
#if defined(AK_OS_WINDOWS)
    VERIFY(m_fd != INVALID_SOCKET);
#else
    VERIFY(m_fd >= 0);
#endif
}

UDPServer::~UDPServer()
{
#if defined(AK_OS_WINDOWS)
    ::closesocket(m_fd);
#else
    ::close(m_fd);
#endif
}

bool UDPServer::bind(IPv4Address const& address, u16 port)
{
    if (m_bound)
        return false;

    auto saddr = SocketAddress(address, port);
    auto in = saddr.to_sockaddr_in();

    if (Core::System::bind(m_fd, (sockaddr const*)&in, sizeof(in)).is_error()) {
        perror("UDPServer::bind");
        return false;
    }

    m_bound = true;

    m_notifier = Notifier::construct(m_fd, Notifier::Type::Read, this);
    m_notifier->on_activation = [this] {
        if (on_ready_to_receive)
            on_ready_to_receive();
    };
    return true;
}

ErrorOr<ByteBuffer> UDPServer::receive(size_t size, sockaddr_in& in)
{
    auto buf = TRY(ByteBuffer::create_uninitialized(size));
    socklen_t in_len = sizeof(in);
    auto bytes_received = TRY(Core::System::recvfrom(m_fd, buf.data(), size, 0, (sockaddr*)&in, &in_len));
    buf.resize(bytes_received);
    return buf;
}

Optional<IPv4Address> UDPServer::local_address() const
{
#if defined(AK_OS_WINDOWS)
    if (m_fd == INVALID_SOCKET)
        return {};
#else
    if (m_fd == -1)
        return {};
#endif

    sockaddr_in address;
    socklen_t len = sizeof(address);
    if (Core::System::getsockname(m_fd, (sockaddr*)&address, &len).is_error())
        return {};

    return IPv4Address(address.sin_addr.s_addr);
}

Optional<u16> UDPServer::local_port() const
{
#if defined(AK_OS_WINDOWS)
    if (m_fd == INVALID_SOCKET)
        return {};
#else
    if (m_fd == -1)
        return {};
#endif

    sockaddr_in address;
    socklen_t len = sizeof(address);
    if (Core::System::getsockname(m_fd, (sockaddr*)&address, &len).is_error())
        return {};

    return ntohs(address.sin_port);
}

ErrorOr<size_t> UDPServer::send(ReadonlyBytes buffer, sockaddr_in const& to)
{
#if defined(AK_OS_WINDOWS)
    if (m_fd == INVALID_SOCKET)
        return Error::from_errno(EBADF);
#else
    if (m_fd < 0)
        return Error::from_errno(EBADF);
#endif

    return TRY(Core::System::sendto(m_fd, buffer.data(), buffer.size(), 0, (sockaddr const*)&to, sizeof(to)));
}

}
