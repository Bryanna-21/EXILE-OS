/*
 * Copyright (c) 2022, the SerenityOS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <Kernel/Devices/GPU/Console/BootFramebufferConsole.h>
#include <Kernel/Locking/Spinlock.h>
#include <Kernel/Memory/MemoryManager.h>

namespace Kernel::Graphics {

BootFramebufferConsole::BootFramebufferConsole(PhysicalAddress framebuffer_addr, size_t width, size_t height, size_t pitch)
    : GenericFramebufferConsoleImpl(width, height, pitch)
{
    // NOTE: We're very early in the boot process, memory allocations shouldn't really fail
    auto framebuffer_end = Memory::page_round_up(framebuffer_addr.offset(height * pitch).get()).release_value();
    m_framebuffer = MM.allocate_mmio_kernel_region(framebuffer_addr.page_base(), framebuffer_end - framebuffer_addr.page_base().get(), "Boot Framebuffer"sv, Memory::Region::Access::ReadWrite, Memory::MemoryType::NonCacheable).release_value();

    m_framebuffer_data = m_framebuffer->vaddr().offset(framebuffer_addr.offset_in_page()).as_ptr();
    memset(m_framebuffer_data, 0, height * pitch);
}

void BootFramebufferConsole::clear(size_t x, size_t y, size_t length)
{
    if (m_splash_owns_screen.load(AK::memory_order_acquire))
        return;
    SpinlockLocker lock(m_lock);
    if (m_framebuffer_data && !m_splash_owns_screen.load(AK::memory_order_acquire))
        GenericFramebufferConsoleImpl::clear(x, y, length);
}

void BootFramebufferConsole::clear_glyph(size_t x, size_t y)
{

    VERIFY(m_lock.is_locked());
    GenericFramebufferConsoleImpl::clear_glyph(x, y);
}

void BootFramebufferConsole::enable()
{
    // Once disabled, ignore requests to re-enable
}

void BootFramebufferConsole::disable()
{
    SpinlockLocker lock(m_lock);
    // The framebuffer is about to be handed to the next owner; the splash must not draw on it anymore.
    m_splash_state.end(BootSplash::EndReason::Handoff);
    m_splash_owns_screen.store(false, AK::memory_order_release);
    GenericFramebufferConsoleImpl::disable();
    m_framebuffer = nullptr;
    m_framebuffer_data = nullptr;
}

void BootFramebufferConsole::write(size_t x, size_t y, char ch, Color background, Color foreground, bool critical)
{
    if (critical) {
        InterruptsState previous_interrupts_state;
        if (!m_lock.try_lock(previous_interrupts_state))
            return;

        // Fatal output must not wait for the framebuffer lock. If we acquired it,
        // end the splash under the lock before drawing diagnostic text.
        if (m_splash_owns_screen.load(AK::memory_order_acquire)) {
            if (m_splash_state.end(BootSplash::EndReason::FatalError) && m_framebuffer_data)
                BootSplash::render_clear(splash_surface(), m_splash_state);
            m_splash_owns_screen.store(false, AK::memory_order_release);
        }

        if (m_framebuffer_data && !m_splash_owns_screen.load(AK::memory_order_acquire))
            GenericFramebufferConsoleImpl::write(x, y, ch, background, foreground, critical);

        m_lock.unlock(previous_interrupts_state);
        return;
    }

    // Kernel messages remain logged even while the splash owns the framebuffer.
    if (m_splash_owns_screen.load(AK::memory_order_acquire))
        return;

    SpinlockLocker lock(m_lock);
    // Recheck after locking in case the splash started while we were waiting.
    if (m_framebuffer_data && !m_splash_owns_screen.load(AK::memory_order_acquire))
        GenericFramebufferConsoleImpl::write(x, y, ch, background, foreground, critical);
}

void BootFramebufferConsole::set_cursor(size_t x, size_t y)
{
    if (m_splash_owns_screen.load(AK::memory_order_acquire))
        return;
    // Note: To ensure we don't trigger a deadlock, let's assert in
    // case we already locked the spinlock, so we know there's a bug
    // in the call path.
    VERIFY(!m_lock.is_locked());
    SpinlockLocker lock(m_lock);
    hide_cursor();
    m_x = x;
    m_y = y;
    show_cursor();
}

void BootFramebufferConsole::hide_cursor()
{
    VERIFY(m_lock.is_locked());
    if (m_splash_owns_screen.load(AK::memory_order_acquire))
        return;
    GenericFramebufferConsoleImpl::hide_cursor();
}

void BootFramebufferConsole::show_cursor()
{
    VERIFY(m_lock.is_locked());
    if (m_splash_owns_screen.load(AK::memory_order_acquire))
        return;
    GenericFramebufferConsoleImpl::show_cursor();
}

BootSplash::Surface BootFramebufferConsole::splash_surface() const
{
    return { m_framebuffer_data, m_pitch, static_cast<u32>(width()), static_cast<u32>(height()) };
}

void BootFramebufferConsole::splash_begin()
{
    SpinlockLocker lock(m_lock);
    if (!m_framebuffer_data)
        return;

    auto surface = splash_surface();
    // If the framebuffer is unusable or too small/large, stay a plain text console.
    if (!surface.is_usable() || !m_splash_state.begin(surface.width, surface.height))
        return;

    // The constructor already cleared the screen, so only the splash content needs drawing.
    m_splash_owns_screen.store(true, AK::memory_order_release);
    BootSplash::render_full(surface, m_splash_state);
}

void BootFramebufferConsole::splash_milestone(BootSplash::Milestone milestone)
{
    if (!m_splash_owns_screen.load(AK::memory_order_acquire))
        return;
    SpinlockLocker lock(m_lock);
    if (!m_framebuffer_data)
        return;
    u8 const previously_completed = m_splash_state.completed;
    if (m_splash_state.advance(milestone))
        BootSplash::render_progress(splash_surface(), m_splash_state, previously_completed);
}

void BootFramebufferConsole::splash_end(BootSplash::EndReason reason)
{
    // Cheap early-out: this is called for every character of fatal output.
    if (!m_splash_owns_screen.load(AK::memory_order_acquire))
        return;

    if (reason == BootSplash::EndReason::FatalError) {
        // Fatal output must never block waiting for the framebuffer lock.
        InterruptsState previous_interrupts_state;
        if (!m_lock.try_lock(previous_interrupts_state))
            return;

        if (m_splash_state.end(reason)) {
            // Clear the splash before allowing fatal diagnostic text through.
            if (m_framebuffer_data)
                BootSplash::render_clear(splash_surface(), m_splash_state);
            m_splash_owns_screen.store(false, AK::memory_order_release);
        }

        m_lock.unlock(previous_interrupts_state);
        return;
    }

    SpinlockLocker lock(m_lock);
    if (!m_splash_state.end(reason))
        return;
    // Leave a clean black screen behind before handing over the console.
    if (m_framebuffer_data)
        BootSplash::render_clear(splash_surface(), m_splash_state);
    m_splash_owns_screen.store(false, AK::memory_order_release);
}

u8* BootFramebufferConsole::framebuffer_data()
{
    VERIFY(m_lock.is_locked());
    VERIFY(m_framebuffer_data);
    return m_framebuffer_data;
}

}
