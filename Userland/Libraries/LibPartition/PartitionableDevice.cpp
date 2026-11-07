/*
 * Copyright (c) 2023, Ben Wiederhake <BenWiederhake.GitHub@gmx.de>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/NumericLimits.h>
#include <LibPartition/PartitionableDevice.h>

#ifndef KERNEL
#    include <sys/ioctl.h>
#endif

namespace Partition {

#ifdef KERNEL
ErrorOr<PartitionableDevice> PartitionableDevice::create(Kernel::StorageDevice& device)
{
    if (device.block_size() == 0 || device.max_mathematical_addressable_block() == 0)
        return Error::from_errno(EINVAL);
    return PartitionableDevice(device);
}
#else
ErrorOr<PartitionableDevice> PartitionableDevice::create(MaybeOwned<Core::File> device_file)
{
    if (!device_file.ptr())
        return Error::from_errno(EINVAL);

    size_t block_size = 0;
    if (ioctl(device_file->fd(), STORAGE_DEVICE_GET_BLOCK_SIZE, &block_size) < 0)
        return Error::from_errno(errno);
    if (block_size == 0)
        return Error::from_errno(EINVAL);

    u64 size_in_bytes = 0;
    if (ioctl(device_file->fd(), STORAGE_DEVICE_GET_SIZE, &size_in_bytes) < 0)
        return Error::from_errno(errno);
    if (size_in_bytes == 0 || size_in_bytes % block_size != 0)
        return Error::from_errno(EINVAL);

    return PartitionableDevice(move(device_file), block_size, size_in_bytes / block_size);
}
#endif

#ifdef KERNEL
PartitionableDevice::PartitionableDevice(Kernel::StorageDevice& device)
    : m_device(device)
{
}
#else
PartitionableDevice::PartitionableDevice(MaybeOwned<Core::File> device_file, size_t block_size, u64 block_count)
    : m_device_file(move(device_file))
    , m_block_size(block_size)
    , m_block_count(block_count)
{
}
#endif

#ifdef KERNEL
PartitionableDevice PartitionableDevice::clone_unowned()
{
    return PartitionableDevice(m_device);
}
#else
PartitionableDevice PartitionableDevice::clone_unowned()
{
    return PartitionableDevice(MaybeOwned<Core::File>(*m_device_file), m_block_size, m_block_count);
}
#endif

#ifdef KERNEL
ErrorOr<PartitionableDevice> PartitionableDevice::clone_owned()
{
    return PartitionableDevice(m_device);
}
#else
ErrorOr<PartitionableDevice> PartitionableDevice::clone_owned()
{
    auto cloned_file = TRY(Core::File::adopt_fd(m_device_file->fd(), Core::File::OpenMode::Read, Core::File::ShouldCloseFileDescriptor::No));
    return PartitionableDevice(move(cloned_file), m_block_size, m_block_count);
}
#endif

#ifdef KERNEL
size_t PartitionableDevice::block_size() const
{
    return m_device.block_size();
}

u64 PartitionableDevice::block_count() const
{
    return m_device.max_mathematical_addressable_block();
}

ErrorOr<void> PartitionableDevice::read_block(size_t block_index, Bytes block_buffer)
{
    if (block_buffer.size() != block_size() || static_cast<u64>(block_index) >= block_count())
        return Error::from_errno(EINVAL);
    auto buffer = UserOrKernelBuffer::for_kernel_buffer(block_buffer.data());
    if (!m_device.read_block(block_index, buffer))
        return Error::from_errno(EIO);
    return {};
}
#else
size_t PartitionableDevice::block_size() const
{
    return m_block_size;
}

u64 PartitionableDevice::block_count() const
{
    return m_block_count;
}

ErrorOr<void> PartitionableDevice::read_block(size_t block_index, Bytes block_buffer)
{
    if (block_buffer.size() != block_size() || static_cast<u64>(block_index) >= m_block_count)
        return Error::from_errno(EINVAL);
    if (block_index > static_cast<size_t>(NumericLimits<off_t>::max()) / block_size())
        return Error::from_errno(EOVERFLOW);
    TRY(m_device_file->seek(static_cast<off_t>(block_index * block_size()), SeekMode::SetPosition));
    TRY(m_device_file->read_until_filled(block_buffer));
    return {};
}
#endif

}
