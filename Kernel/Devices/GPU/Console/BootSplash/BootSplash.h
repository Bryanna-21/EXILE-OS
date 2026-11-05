/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Early boot splash for Exile OS.
//
// This header contains only the pure, allocation-free parts of the splash: layout math, the
// progress state machine and the static glyph/emblem data. It deliberately depends on AK alone
// (no Kernel headers, no locks, no heap) so that it is safe in early boot and can be unit tested.
// Drawing lives in BootSplash.cpp; owning the screen lives in BootFramebufferConsole.

#include <AK/StringView.h>
#include <AK/Types.h>

namespace Kernel::Graphics::BootSplash {

// Real startup checkpoints. A milestone is only reported from a call site in the kernel init
// sequence once the corresponding work has actually completed. Milestones may be skipped on
// some architectures; they can never go backwards.
enum class Milestone : u8 {
    KernelStarted,
    InterruptsConfigured,
    HardwareDetected,
    TimersReady,
    SchedulerReady,
    BusesProbed,
    PreparingGraphics,
};
static constexpr u8 milestone_count = static_cast<u8>(Milestone::PreparingGraphics) + 1;

enum class EndReason : u8 {
    // The framebuffer is being handed to the next owner (graphics subsystem or text console).
    Handoff,
    // A fatal error is being printed; the screen must show diagnostics.
    FatalError,
};

enum class Phase : u8 {
    Inactive,
    Active,
    Ended,
};

constexpr StringView milestone_label(Milestone milestone)
{
    switch (milestone) {
    case Milestone::KernelStarted:
        return "STARTING KERNEL"sv;
    case Milestone::InterruptsConfigured:
        return "INTERRUPTS CONFIGURED"sv;
    case Milestone::HardwareDetected:
        return "HARDWARE DETECTED"sv;
    case Milestone::TimersReady:
        return "TIMERS READY"sv;
    case Milestone::SchedulerReady:
        return "SCHEDULER READY"sv;
    case Milestone::BusesProbed:
        return "BUSES PROBED"sv;
    case Milestone::PreparingGraphics:
        return "PREPARING GRAPHICS"sv;
    }
    return ""sv;
}

// The longest label above. Used to reserve the status box so layout never depends on progress.
static constexpr u32 max_status_characters = 21;

// Palette (0x00RRGGBB, which is BGRx8888 in little-endian memory). No other colors are ever drawn.
namespace Palette {
static constexpr u32 Black = 0x000000;
static constexpr u32 White = 0xFFFFFF;
static constexpr u32 Green = 0x22C55E;
static constexpr u32 Blue = 0x3B82F6;
static constexpr u32 DimBlue = 0x12305A;
}

struct Rect {
    u32 x { 0 };
    u32 y { 0 };
    u32 width { 0 };
    u32 height { 0 };

    constexpr u64 right() const { return static_cast<u64>(x) + width; }
    constexpr u64 bottom() const { return static_cast<u64>(y) + height; }
    constexpr bool is_empty() const { return width == 0 || height == 0; }
    constexpr bool intersects(Rect const& other) const
    {
        return x < other.right() && other.x < right() && y < other.bottom() && other.y < bottom();
    }
};

// ---- Emblem -------------------------------------------------------------------------------
//
// PLACEHOLDER ARTWORK: the repository does not currently contain an approved Exile emblem, so
// this is a deliberately simple geometric mark (blue frame around a green "E") expressed as
// axis-aligned rectangles on a 24x24 grid. Replace emblem_rects with the approved artwork's
// rectangle decomposition (or swap the emblem drawing for a bitmap) without touching anything else.

static constexpr u32 emblem_grid = 24;
static constexpr u8 emblem_part_count = 5;

enum class EmblemColor : u8 {
    Blue,
    Green,
};

struct EmblemRect {
    u8 x;
    u8 y;
    u8 width;
    u8 height;
    u8 part; // Revealed once this many parts are visible (0-based).
    EmblemColor color;
};

static constexpr EmblemRect emblem_rects[] = {
    // Part 0: frame.
    { 0, 0, 24, 2, 0, EmblemColor::Blue },
    { 0, 22, 24, 2, 0, EmblemColor::Blue },
    { 0, 2, 2, 20, 0, EmblemColor::Blue },
    { 22, 2, 2, 20, 0, EmblemColor::Blue },
    // Parts 1-4: the "E", traced spine first, then top, middle and bottom bars.
    { 7, 7, 3, 11, 1, EmblemColor::Green },
    { 10, 7, 8, 3, 2, EmblemColor::Green },
    { 10, 11, 6, 3, 3, EmblemColor::Green },
    { 10, 15, 8, 3, 4, EmblemColor::Green },
};

// ---- Font ---------------------------------------------------------------------------------
//
// Minimal built-in 5x7 uppercase font, used for the wordmark and the status line. It is
// self-contained so the splash never depends on downloaded or filesystem-provided fonts.

static constexpr u32 glyph_columns = 5;
static constexpr u32 glyph_rows = 7;
static constexpr u32 glyph_advance = glyph_columns + 1; // One column of spacing.

// One byte per row, bit 4 is the leftmost column.
static constexpr u8 glyph_table[26][glyph_rows] = {
    { 0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001 }, // A
    { 0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110 }, // B
    { 0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110 }, // C
    { 0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110 }, // D
    { 0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111 }, // E
    { 0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000 }, // F
    { 0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111 }, // G
    { 0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001 }, // H
    { 0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110 }, // I
    { 0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100 }, // J
    { 0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001 }, // K
    { 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111 }, // L
    { 0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001 }, // M
    { 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001 }, // N
    { 0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110 }, // O
    { 0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000 }, // P
    { 0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101 }, // Q
    { 0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001 }, // R
    { 0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110 }, // S
    { 0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100 }, // T
    { 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110 }, // U
    { 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100 }, // V
    { 0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010 }, // W
    { 0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001 }, // X
    { 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100 }, // Y
    { 0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111 }, // Z
};

// Returns the seven row bytes for a letter, or nullptr for anything that renders as blank (space, unsupported).
constexpr u8 const* glyph_for(char ch)
{
    if (ch >= 'a' && ch <= 'z')
        ch = static_cast<char>(ch - 'a' + 'A');
    if (ch < 'A' || ch > 'Z')
        return nullptr;
    return glyph_table[ch - 'A'];
}

static constexpr StringView wordmark_text = "EXILE OS"sv;

// Width in pixels of `characters` glyph cells at `scale`, without the trailing spacing column.
constexpr u64 text_width(size_t characters, u32 scale)
{
    if (characters == 0)
        return 0;
    return (static_cast<u64>(characters) * glyph_advance - 1) * scale;
}

// ---- Layout -------------------------------------------------------------------------------

struct Layout {
    bool valid { false };
    u32 cell { 0 };
    u32 wordmark_scale { 0 };
    u32 status_scale { 0 };
    Rect emblem {};
    Rect wordmark {};
    Rect progress {};
    u32 segment_width { 0 };
    u32 segment_gap { 0 };
    Rect status {};
    // Smallest rectangle containing everything the splash ever draws.
    Rect bounds {};
};

static constexpr u32 minimum_display_dimension = 64;
static constexpr u32 maximum_display_dimension = 1u << 15;

constexpr u32 max_u32(u32 a, u32 b) { return a > b ? a : b; }
constexpr u32 min_u32(u32 a, u32 b) { return a < b ? a : b; }

// Computes a centered, integer-scaled layout. Returns an invalid layout when the display is too
// small or too large for the splash; callers must then leave the text console alone.
constexpr Layout compute_layout(u32 display_width, u32 display_height)
{
    Layout layout;
    if (display_width < minimum_display_dimension || display_height < minimum_display_dimension)
        return layout;
    if (display_width > maximum_display_dimension || display_height > maximum_display_dimension)
        return layout;

    u32 const cell = max_u32(1, min_u32(display_width, display_height) / 120);
    u32 const wordmark_scale = max_u32(1, cell * 2 / 3);
    u32 const status_scale = max_u32(1, wordmark_scale / 2);

    u32 const emblem_size = emblem_grid * cell;
    u32 const wordmark_width = static_cast<u32>(text_width(wordmark_text.length(), wordmark_scale));
    u32 const wordmark_height = glyph_rows * wordmark_scale;

    u32 const segment_width = 6 * cell;
    u32 const segment_gap = cell;
    u32 const progress_width = milestone_count * segment_width + (milestone_count - 1u) * segment_gap;
    u32 const progress_height = max_u32(2, cell);

    u32 const status_width = static_cast<u32>(text_width(max_status_characters, status_scale));
    u32 const status_height = glyph_rows * status_scale;

    u32 const gap_large = 6 * cell;
    u32 const gap_small = 3 * cell;

    u64 const total_height = static_cast<u64>(emblem_size) + gap_large + wordmark_height + gap_large + progress_height + gap_small + status_height;
    u32 const widest = max_u32(max_u32(emblem_size, wordmark_width), max_u32(progress_width, status_width));

    if (total_height > display_height || widest > display_width)
        return layout;

    u32 const top = static_cast<u32>((display_height - total_height) / 2);
    auto center_x = [&](u32 width) { return (display_width - width) / 2; };

    u32 y = top;
    layout.emblem = { center_x(emblem_size), y, emblem_size, emblem_size };
    y += emblem_size + gap_large;
    layout.wordmark = { center_x(wordmark_width), y, wordmark_width, wordmark_height };
    y += wordmark_height + gap_large;
    layout.progress = { center_x(progress_width), y, progress_width, progress_height };
    y += progress_height + gap_small;
    layout.status = { center_x(status_width), y, status_width, status_height };

    layout.cell = cell;
    layout.wordmark_scale = wordmark_scale;
    layout.status_scale = status_scale;
    layout.segment_width = segment_width;
    layout.segment_gap = segment_gap;

    u32 const bounds_x = center_x(widest);
    layout.bounds = { bounds_x, top, widest, static_cast<u32>(total_height) };
    layout.valid = true;
    return layout;
}

// ---- Reveal rules (all driven by completed milestones, never by wall-clock time) ----------

constexpr u8 emblem_parts_visible(u8 completed)
{
    return completed < emblem_part_count ? completed : emblem_part_count;
}

// The wordmark is revealed two characters at a time as startup advances.
constexpr size_t wordmark_characters_visible(u8 completed)
{
    if (completed < 2)
        return 0;
    size_t const characters = static_cast<size_t>(completed - 1) * 2;
    return characters < wordmark_text.length() ? characters : wordmark_text.length();
}

// ---- State machine ------------------------------------------------------------------------

struct State {
    Phase phase { Phase::Inactive };
    EndReason end_reason { EndReason::Handoff };
    // Number of milestones reached, counting skipped ones. Never decreases.
    u8 completed { 0 };
    Layout layout {};

    constexpr bool is_active() const { return phase == Phase::Active; }

    // Text must only be drawn on the framebuffer while the splash is not on screen.
    constexpr bool text_output_allowed() const { return phase != Phase::Active; }

    // Inactive -> Active. Fails (and stays Inactive) when the display cannot host the splash.
    constexpr bool begin(u32 display_width, u32 display_height)
    {
        if (phase != Phase::Inactive)
            return false;
        auto computed = compute_layout(display_width, display_height);
        if (!computed.valid)
            return false;
        layout = computed;
        completed = 1; // KernelStarted is implied by beginning.
        phase = Phase::Active;
        return true;
    }

    // Returns true if visible progress changed. Late or duplicate milestones are ignored.
    constexpr bool advance(Milestone milestone)
    {
        if (phase != Phase::Active)
            return false;
        u8 const reached = static_cast<u8>(milestone) + 1;
        if (reached <= completed)
            return false;
        completed = reached;
        return true;
    }

    // Active -> Ended. Idempotent. Returns true if this call ended an active splash.
    constexpr bool end(EndReason reason)
    {
        if (phase != Phase::Active)
            return false;
        phase = Phase::Ended;
        end_reason = reason;
        return true;
    }

    constexpr Milestone current_milestone() const
    {
        return static_cast<Milestone>(completed == 0 ? 0 : completed - 1);
    }
};

// ---- Rendering ----------------------------------------------------------------------------

// A BGRx8888 pixel surface. `pitch` is in bytes.
struct Surface {
    u8* data { nullptr };
    size_t pitch { 0 };
    u32 width { 0 };
    u32 height { 0 };

    // The renderer stores whole 32-bit pixels, so the base and pitch must be 4-byte aligned.
    bool is_usable() const
    {
        if (!data || width == 0 || height == 0)
            return false;
        if (pitch < static_cast<size_t>(width) * 4)
            return false;
        return ((reinterpret_cast<FlatPtr>(data) | pitch) & 3) == 0;
    }
};

// Draws everything for the state's current progress from scratch (does not clear the screen).
void render_full(Surface const&, State const&);

// Draws only what changed between `previously_completed` and the state's current progress.
void render_progress(Surface const&, State const&, u8 previously_completed);

// Blanks the splash's bounding box so that diagnostics can be printed over a clean background.
void render_clear(Surface const&, State const&);

}
