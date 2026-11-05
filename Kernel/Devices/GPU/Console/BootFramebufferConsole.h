/*
 * Copyright (c) 2022, the SerenityOS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <AK/Atomic.h>
#include <Kernel/Devices/GPU/Console/BootSplash/BootSplash.h>
#include <Kernel/Devices/GPU/Console/GenericFramebufferConsole.h>
#include <Kernel/Forward.h>

namespace Kernel::Graphics {

class BootFramebufferConsole : public GenericFramebufferConsoleImpl {
public:
    virtual void clear(size_t x, size_t y, size_t length) override;
    virtual void write(size_t x, size_t y, char ch, Color background, Color foreground, bool critical = false) override;
    using GenericFramebufferConsoleImpl::write;

    virtual void enable() override;
    virtual void disable() override;

    virtual void flush(size_t, size_t, size_t, size_t) override { }
    virtual void set_resolution(size_t, size_t, size_t) override { }

    // While the splash is on screen this console owns the framebuffer for the splash and swallows
    // all text output. The splash ends on handoff (disable()) or as soon as a fatal error is printed.
    virtual void splash_begin() override;
    virtual void splash_milestone(BootSplash::Milestone) override;
    virtual void splash_end(BootSplash::EndReason) override;

    u8* unsafe_framebuffer_data() { return m_framebuffer_data; }

    BootFramebufferConsole(PhysicalAddress framebuffer_addr, size_t width, size_t height, size_t pitch);

private:
    virtual void set_cursor(size_t x, size_t y) override;
    virtual void hide_cursor() override;
    virtual void show_cursor() override;

protected:
    virtual void clear_glyph(size_t x, size_t y) override;

    virtual u8* framebuffer_data() override;

    BootSplash::Surface splash_surface() const;

    OwnPtr<Memory::Region> m_framebuffer;
    u8* m_framebuffer_data {};
    mutable Spinlock<LockRank::None> m_lock {};

    // The splash state is only modified with m_lock held. m_splash_owns_screen mirrors "splash is
    // active" so that text output and the fatal-error path can check it without taking the lock.
    BootSplash::State m_splash_state {};
    Atomic<bool> m_splash_owns_screen { false };
};

}
