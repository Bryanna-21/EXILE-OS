/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/Vector.h>
#include <Kernel/Devices/GPU/Console/BootSplash/BootSplash.h>
#include <LibTest/TestCase.h>

#include <Kernel/Devices/GPU/Console/BootSplash/BootSplash.cpp>

using namespace Kernel::Graphics::BootSplash;

namespace {

constexpr u32 sentinel = 0xDEADBEEF;

// A pixel buffer surrounded by guard rows and padding columns, to catch any out-of-bounds write.
struct TestFramebuffer {
    TestFramebuffer(u32 width, u32 height, u32 padding_columns = 3)
        : width(width)
        , height(height)
        , stride_pixels(width + padding_columns)
    {
        pixels.resize(static_cast<size_t>(stride_pixels) * (height + 2));
        for (auto& pixel : pixels)
            pixel = sentinel;
        // Make the visible area black, like the real boot console does when it is created.
        for (u32 y = 0; y < height; ++y) {
            for (u32 x = 0; x < width; ++x)
                at(x, y) = Palette::Black;
        }
    }

    u32& at(u32 x, u32 y) { return pixels[static_cast<size_t>(y + 1) * stride_pixels + x]; }

    Surface surface()
    {
        return { reinterpret_cast<u8*>(&pixels[stride_pixels]), static_cast<size_t>(stride_pixels) * 4, width, height };
    }

    bool guards_intact()
    {
        for (u32 x = 0; x < stride_pixels; ++x) {
            if (pixels[x] != sentinel || pixels[static_cast<size_t>(height + 1) * stride_pixels + x] != sentinel)
                return false;
        }
        for (u32 y = 0; y < height; ++y) {
            for (u32 x = width; x < stride_pixels; ++x) {
                if (at(x, y) != sentinel)
                    return false;
            }
        }
        return true;
    }

    bool only_palette_colors()
    {
        for (u32 y = 0; y < height; ++y) {
            for (u32 x = 0; x < width; ++x) {
                u32 const pixel = at(x, y);
                if (pixel != Palette::Black && pixel != Palette::White && pixel != Palette::Green && pixel != Palette::Blue && pixel != Palette::DimBlue)
                    return false;
            }
        }
        return true;
    }

    size_t count(u32 color)
    {
        size_t total = 0;
        for (u32 y = 0; y < height; ++y) {
            for (u32 x = 0; x < width; ++x)
                total += at(x, y) == color;
        }
        return total;
    }

    size_t count_in(Rect const& rect, u32 color)
    {
        size_t total = 0;
        for (u32 y = rect.y; y < rect.y + rect.height && y < height; ++y) {
            for (u32 x = rect.x; x < rect.x + rect.width && x < width; ++x)
                total += at(x, y) == color;
        }
        return total;
    }

    u32 width;
    u32 height;
    u32 stride_pixels;
    Vector<u32> pixels;
};

// Hue in whole degrees (0-359) of an RGB color; 0 for greys.
constexpr int hue_degrees(u32 color)
{
    int const red = (color >> 16) & 0xff;
    int const green = (color >> 8) & 0xff;
    int const blue = color & 0xff;
    int const max_channel = red > green ? (red > blue ? red : blue) : (green > blue ? green : blue);
    int const min_channel = red < green ? (red < blue ? red : blue) : (green < blue ? green : blue);
    int const delta = max_channel - min_channel;
    if (delta == 0)
        return 0;
    int hue = 0;
    if (max_channel == red)
        hue = 60 * (green - blue) / delta;
    else if (max_channel == green)
        hue = 120 + 60 * (blue - red) / delta;
    else
        hue = 240 + 60 * (red - green) / delta;
    return hue < 0 ? hue + 360 : hue;
}

constexpr bool is_greyscale(u32 color)
{
    return ((color >> 16) & 0xff) == ((color >> 8) & 0xff) && ((color >> 8) & 0xff) == (color & 0xff);
}

struct DisplaySize {
    u32 width;
    u32 height;
};

constexpr DisplaySize supported_displays[] = {
    { 640, 480 },
    { 800, 600 },
    { 1024, 768 },
    { 1280, 720 },
    { 1366, 768 },
    { 1920, 1080 },
    { 2560, 1600 },
    { 3440, 1440 }, // Ultrawide.
    { 3840, 2160 },
    { 1080, 1920 }, // Portrait.
    { 128, 128 },   // Very small but still valid.
};

// Pixel-level tests allocate whole frames, so they use a smaller (but still varied) set of displays.
constexpr DisplaySize rendered_displays[] = {
    { 640, 480 },
    { 1024, 768 },
    { 1280, 720 },
    { 1920, 1080 },
    { 1080, 1920 }, // Portrait.
    { 128, 128 },
};

}

// The official palette is black, white, green and blue. Pin the hues so that nothing purple
// (hue ~260-300) or otherwise off-palette can sneak in through a tweaked constant.
static_assert(Palette::Black == 0x000000);
static_assert(Palette::White == 0xFFFFFF);
static_assert(is_greyscale(Palette::Black) && is_greyscale(Palette::White));
static_assert(!is_greyscale(Palette::Green) && hue_degrees(Palette::Green) >= 120 && hue_degrees(Palette::Green) <= 160);
static_assert(!is_greyscale(Palette::Blue) && hue_degrees(Palette::Blue) >= 205 && hue_degrees(Palette::Blue) <= 230);
static_assert(!is_greyscale(Palette::DimBlue) && hue_degrees(Palette::DimBlue) >= 205 && hue_degrees(Palette::DimBlue) <= 230);

// ---- Asset and dimension validation ----

TEST_CASE(status_labels_fit_reserved_status_box)
{
    for (u8 index = 0; index < milestone_count; ++index) {
        auto label = milestone_label(static_cast<Milestone>(index));
        EXPECT(!label.is_empty());
        EXPECT(label.length() <= max_status_characters);
        for (auto ch : label)
            EXPECT(ch == ' ' || glyph_for(ch) != nullptr);
    }
    // The reservation is tight: some label uses all of it.
    size_t longest = 0;
    for (u8 index = 0; index < milestone_count; ++index)
        longest = max(longest, milestone_label(static_cast<Milestone>(index)).length());
    EXPECT_EQ(longest, static_cast<size_t>(max_status_characters));
}

TEST_CASE(wordmark_is_exile_os_and_renderable)
{
    EXPECT_EQ(wordmark_text, "EXILE OS"sv);
    for (auto ch : wordmark_text)
        EXPECT(ch == ' ' || glyph_for(ch) != nullptr);
}

TEST_CASE(glyph_table_is_well_formed)
{
    for (char ch = 'A'; ch <= 'Z'; ++ch) {
        auto const* rows = glyph_for(ch);
        VERIFY(rows);
        bool any_pixel = false;
        for (u32 row = 0; row < glyph_rows; ++row) {
            EXPECT(rows[row] < (1u << glyph_columns));
            any_pixel |= rows[row] != 0;
        }
        EXPECT(any_pixel);
        EXPECT_EQ(glyph_for(static_cast<char>(ch - 'A' + 'a')), rows);
    }
    EXPECT(glyph_for(' ') == nullptr);
    EXPECT(glyph_for('?') == nullptr);
    EXPECT(glyph_for('\0') == nullptr);
}

TEST_CASE(glyphs_are_distinct)
{
    for (char a = 'A'; a <= 'Z'; ++a) {
        for (char b = a + 1; b <= 'Z'; ++b) {
            bool same = true;
            for (u32 row = 0; row < glyph_rows; ++row)
                same &= glyph_for(a)[row] == glyph_for(b)[row];
            EXPECT(!same);
        }
    }
}

TEST_CASE(emblem_rects_stay_inside_grid_and_cover_all_parts)
{
    bool part_seen[emblem_part_count] = {};
    for (auto const& rect : emblem_rects) {
        EXPECT(rect.width > 0 && rect.height > 0);
        EXPECT(static_cast<u32>(rect.x) + rect.width <= emblem_grid);
        EXPECT(static_cast<u32>(rect.y) + rect.height <= emblem_grid);
        EXPECT(rect.part < emblem_part_count);
        part_seen[rect.part] = true;
    }
    for (auto seen : part_seen)
        EXPECT(seen);
}

// ---- Layout calculations ----

TEST_CASE(layout_is_valid_centered_and_inside_display_for_supported_sizes)
{
    for (auto const& display : supported_displays) {
        auto layout = compute_layout(display.width, display.height);
        EXPECT(layout.valid);

        Rect const* elements[] = { &layout.emblem, &layout.wordmark, &layout.progress, &layout.status };
        for (auto const* element : elements) {
            EXPECT(!element->is_empty());
            EXPECT(element->right() <= display.width);
            EXPECT(element->bottom() <= display.height);
            // Horizontally centered, within the rounding of one pixel.
            i64 const left = element->x;
            i64 const right = static_cast<i64>(display.width) - static_cast<i64>(element->right());
            EXPECT(left - right <= 1 && right - left <= 1);
        }

        // Nothing overlaps and the stack reads top to bottom.
        EXPECT(layout.emblem.bottom() <= layout.wordmark.y);
        EXPECT(layout.wordmark.bottom() <= layout.progress.y);
        EXPECT(layout.progress.bottom() <= layout.status.y);

        // Vertically centered as a block.
        i64 const top_margin = layout.bounds.y;
        i64 const bottom_margin = static_cast<i64>(display.height) - static_cast<i64>(layout.bounds.bottom());
        EXPECT(top_margin - bottom_margin <= 1 && bottom_margin - top_margin <= 1);
    }
}

TEST_CASE(layout_scales_without_distortion)
{
    for (auto const& display : supported_displays) {
        auto layout = compute_layout(display.width, display.height);
        EXPECT(layout.valid);
        // The emblem is square and an exact integer multiple of its grid: crisp, undistorted.
        EXPECT_EQ(layout.emblem.width, layout.emblem.height);
        EXPECT_EQ(layout.emblem.width % emblem_grid, 0u);
        EXPECT_EQ(layout.emblem.width / emblem_grid, layout.cell);
        EXPECT_EQ(layout.wordmark.height, glyph_rows * layout.wordmark_scale);
        EXPECT_EQ(layout.wordmark.width, text_width(wordmark_text.length(), layout.wordmark_scale));
    }
}

TEST_CASE(layout_grows_with_display_size)
{
    auto small = compute_layout(800, 600);
    auto large = compute_layout(3840, 2160);
    EXPECT(small.valid && large.valid);
    EXPECT(large.emblem.width > small.emblem.width);
    EXPECT(large.cell > small.cell);
}

TEST_CASE(layout_is_independent_of_progress)
{
    // The reserved boxes never change, so updating progress can never move anything.
    auto before = compute_layout(1920, 1080);
    State state;
    EXPECT(state.begin(1920, 1080));
    for (u8 index = 1; index < milestone_count; ++index)
        state.advance(static_cast<Milestone>(index));
    auto const& after = state.layout;
    EXPECT_EQ(before.emblem.x, after.emblem.x);
    EXPECT_EQ(before.emblem.y, after.emblem.y);
    EXPECT_EQ(before.status.y, after.status.y);
    EXPECT_EQ(before.bounds.height, after.bounds.height);
}

TEST_CASE(layout_rejects_unusable_displays)
{
    EXPECT(!compute_layout(0, 0).valid);
    EXPECT(!compute_layout(0, 1080).valid);
    EXPECT(!compute_layout(1920, 0).valid);
    EXPECT(!compute_layout(63, 63).valid);
    EXPECT(!compute_layout(63, 1080).valid);
    EXPECT(!compute_layout(100, 20).valid);
    EXPECT(!compute_layout(maximum_display_dimension + 1, 1080).valid);
    EXPECT(!compute_layout(1920, maximum_display_dimension + 1).valid);
    EXPECT(!compute_layout(0xffffffffu, 0xffffffffu).valid);
    // The tiny 64x64 case must either fit properly or be rejected, never produce a rect outside the display.
    auto tiny = compute_layout(64, 64);
    if (tiny.valid) {
        EXPECT(tiny.bounds.right() <= 64);
        EXPECT(tiny.bounds.bottom() <= 64);
    }
}

// ---- Startup-state transitions and progress handling ----

TEST_CASE(state_starts_inactive_and_allows_text)
{
    State state;
    EXPECT(state.phase == Phase::Inactive);
    EXPECT(!state.is_active());
    EXPECT(state.text_output_allowed());
    EXPECT_EQ(state.completed, 0);
}

TEST_CASE(state_begin_activates_once)
{
    State state;
    EXPECT(state.begin(1280, 720));
    EXPECT(state.is_active());
    EXPECT(!state.text_output_allowed());
    EXPECT_EQ(state.completed, 1);
    EXPECT(state.current_milestone() == Milestone::KernelStarted);
    EXPECT(!state.begin(1280, 720));
}

TEST_CASE(state_begin_with_unusable_display_stays_inactive)
{
    // Graphics-unavailable fallback: the text console keeps working.
    State state;
    EXPECT(!state.begin(0, 0));
    EXPECT(state.phase == Phase::Inactive);
    EXPECT(state.text_output_allowed());
    EXPECT(!state.advance(Milestone::SchedulerReady));
    EXPECT_EQ(state.completed, 0);
}

TEST_CASE(progress_only_moves_forward)
{
    State state;
    EXPECT(state.begin(1024, 768));
    EXPECT(state.advance(Milestone::HardwareDetected));
    EXPECT_EQ(state.completed, 3);
    // Duplicate and older milestones change nothing.
    EXPECT(!state.advance(Milestone::HardwareDetected));
    EXPECT(!state.advance(Milestone::InterruptsConfigured));
    EXPECT(!state.advance(Milestone::KernelStarted));
    EXPECT_EQ(state.completed, 3);
    EXPECT(state.current_milestone() == Milestone::HardwareDetected);
}

TEST_CASE(progress_tolerates_skipped_milestones)
{
    // Some architectures never report every milestone.
    State state;
    EXPECT(state.begin(1024, 768));
    EXPECT(state.advance(Milestone::SchedulerReady));
    EXPECT_EQ(state.completed, static_cast<u8>(Milestone::SchedulerReady) + 1);
    EXPECT(state.advance(Milestone::PreparingGraphics));
    EXPECT_EQ(state.completed, milestone_count);
    EXPECT(!state.advance(Milestone::PreparingGraphics));
}

TEST_CASE(progress_never_exceeds_total)
{
    State state;
    EXPECT(state.begin(1024, 768));
    for (int round = 0; round < 3; ++round) {
        for (u8 index = 0; index < milestone_count; ++index)
            state.advance(static_cast<Milestone>(index));
    }
    EXPECT_EQ(state.completed, milestone_count);
}

TEST_CASE(advance_before_begin_is_ignored)
{
    State state;
    EXPECT(!state.advance(Milestone::InterruptsConfigured));
    EXPECT_EQ(state.completed, 0);
    EXPECT(state.phase == Phase::Inactive);
}

TEST_CASE(end_is_terminal_and_idempotent)
{
    State state;
    EXPECT(state.begin(1920, 1080));
    EXPECT(state.end(EndReason::Handoff));
    EXPECT(state.phase == Phase::Ended);
    EXPECT(state.end_reason == EndReason::Handoff);
    EXPECT(state.text_output_allowed());
    // A second end, a late milestone and a second begin must all be no-ops.
    EXPECT(!state.end(EndReason::FatalError));
    EXPECT(state.end_reason == EndReason::Handoff);
    EXPECT(!state.advance(Milestone::BusesProbed));
    EXPECT(!state.begin(1920, 1080));
    EXPECT(state.phase == Phase::Ended);
}

TEST_CASE(fatal_error_makes_text_visible)
{
    State state;
    EXPECT(state.begin(1920, 1080));
    EXPECT(!state.text_output_allowed());
    EXPECT(state.end(EndReason::FatalError));
    EXPECT(state.text_output_allowed());
    EXPECT(state.end_reason == EndReason::FatalError);
}

TEST_CASE(reveal_rules_are_monotonic_and_complete_by_the_end)
{
    u8 previous_parts = 0;
    size_t previous_characters = 0;
    for (u8 completed = 0; completed <= milestone_count; ++completed) {
        EXPECT(emblem_parts_visible(completed) >= previous_parts);
        EXPECT(wordmark_characters_visible(completed) >= previous_characters);
        previous_parts = emblem_parts_visible(completed);
        previous_characters = wordmark_characters_visible(completed);
    }
    EXPECT_EQ(emblem_parts_visible(milestone_count), emblem_part_count);
    EXPECT_EQ(wordmark_characters_visible(milestone_count), wordmark_text.length());
    EXPECT_EQ(wordmark_characters_visible(0), 0u);
}

// ---- Rendering ----

TEST_CASE(render_stays_inside_the_surface_and_uses_only_palette_colors)
{
    for (auto const& display : rendered_displays) {
        TestFramebuffer framebuffer(display.width, display.height);
        State state;
        EXPECT(state.begin(display.width, display.height));
        render_full(framebuffer.surface(), state);
        for (u8 index = 1; index < milestone_count; ++index) {
            u8 const previous = state.completed;
            EXPECT(state.advance(static_cast<Milestone>(index)));
            render_progress(framebuffer.surface(), state, previous);
        }
        EXPECT(framebuffer.guards_intact());
        EXPECT(framebuffer.only_palette_colors());
        // Nothing is drawn outside the splash's bounding box.
        for (u32 y = 0; y < display.height; ++y) {
            for (u32 x = 0; x < display.width; ++x) {
                bool const inside = x >= state.layout.bounds.x && x < state.layout.bounds.right() && y >= state.layout.bounds.y && y < state.layout.bounds.bottom();
                if (!inside)
                    EXPECT_EQ(framebuffer.at(x, y), Palette::Black);
            }
        }
    }
}

TEST_CASE(final_frame_contains_emblem_wordmark_progress_and_status)
{
    TestFramebuffer framebuffer(1280, 720);
    State state;
    EXPECT(state.begin(1280, 720));
    render_full(framebuffer.surface(), state);
    for (u8 index = 1; index < milestone_count; ++index) {
        u8 const previous = state.completed;
        state.advance(static_cast<Milestone>(index));
        render_progress(framebuffer.surface(), state, previous);
    }
    auto const& layout = state.layout;

    // Emblem: blue frame and green mark both present.
    EXPECT(framebuffer.count_in(layout.emblem, Palette::Blue) > 0);
    EXPECT(framebuffer.count_in(layout.emblem, Palette::Green) > 0);
    // Wordmark is white, and only white or black.
    EXPECT(framebuffer.count_in(layout.wordmark, Palette::White) > 0);
    EXPECT_EQ(framebuffer.count_in(layout.wordmark, Palette::White) + framebuffer.count_in(layout.wordmark, Palette::Black), static_cast<size_t>(layout.wordmark.width) * layout.wordmark.height);
    // Progress: every segment is lit, so no dim track remains.
    EXPECT_EQ(framebuffer.count_in(layout.progress, Palette::Green), static_cast<size_t>(milestone_count) * layout.segment_width * layout.progress.height);
    EXPECT_EQ(framebuffer.count_in(layout.progress, Palette::DimBlue), 0u);
    // Status text is blue.
    EXPECT(framebuffer.count_in(layout.status, Palette::Blue) > 0);
    // Black background everywhere the splash does not draw.
    EXPECT(framebuffer.count(Palette::Black) > static_cast<size_t>(1280) * 720 / 2);
}

TEST_CASE(progress_segments_match_completed_milestones)
{
    TestFramebuffer framebuffer(1024, 768);
    State state;
    EXPECT(state.begin(1024, 768));
    render_full(framebuffer.surface(), state);
    auto const& layout = state.layout;
    size_t const per_segment = static_cast<size_t>(layout.segment_width) * layout.progress.height;

    for (u8 index = 1; index < milestone_count; ++index) {
        EXPECT_EQ(framebuffer.count_in(layout.progress, Palette::Green), per_segment * state.completed);
        EXPECT_EQ(framebuffer.count_in(layout.progress, Palette::DimBlue), per_segment * (milestone_count - state.completed));
        u8 const previous = state.completed;
        state.advance(static_cast<Milestone>(index));
        render_progress(framebuffer.surface(), state, previous);
    }
    EXPECT_EQ(framebuffer.count_in(layout.progress, Palette::Green), per_segment * milestone_count);
}

TEST_CASE(status_message_is_replaced_not_accumulated)
{
    TestFramebuffer framebuffer(1280, 720);
    State state;
    EXPECT(state.begin(1280, 720));
    render_full(framebuffer.surface(), state);
    size_t const first = framebuffer.count_in(state.layout.status, Palette::Blue);
    EXPECT(first > 0);

    // Render the same state fresh and compare with the incrementally updated one.
    for (u8 index = 1; index < milestone_count; ++index) {
        u8 const previous = state.completed;
        state.advance(static_cast<Milestone>(index));
        render_progress(framebuffer.surface(), state, previous);

        TestFramebuffer fresh(1280, 720);
        render_full(fresh.surface(), state);
        for (u32 y = state.layout.status.y; y < state.layout.status.bottom(); ++y) {
            for (u32 x = state.layout.status.x; x < state.layout.status.right(); ++x)
                EXPECT_EQ(framebuffer.at(x, y), fresh.at(x, y));
        }
    }
}

TEST_CASE(incremental_rendering_matches_a_fresh_render)
{
    for (auto const& display : rendered_displays) {
        TestFramebuffer incremental(display.width, display.height);
        State state;
        EXPECT(state.begin(display.width, display.height));
        render_full(incremental.surface(), state);
        for (u8 index = 1; index < milestone_count; ++index) {
            u8 const previous = state.completed;
            state.advance(static_cast<Milestone>(index));
            render_progress(incremental.surface(), state, previous);
        }

        TestFramebuffer fresh(display.width, display.height);
        render_full(fresh.surface(), state);
        for (u32 y = 0; y < display.height; ++y) {
            for (u32 x = 0; x < display.width; ++x)
                EXPECT_EQ(incremental.at(x, y), fresh.at(x, y));
        }
    }
}

TEST_CASE(render_clear_removes_the_splash_for_diagnostics)
{
    TestFramebuffer framebuffer(1280, 720);
    State state;
    EXPECT(state.begin(1280, 720));
    render_full(framebuffer.surface(), state);
    for (u8 index = 1; index < milestone_count; ++index)
        state.advance(static_cast<Milestone>(index));
    render_full(framebuffer.surface(), state);
    EXPECT(framebuffer.count(Palette::Green) > 0);

    // After a fatal error the splash is cleared, leaving a clean black screen for the panic text.
    EXPECT(state.end(EndReason::FatalError));
    render_clear(framebuffer.surface(), state);
    EXPECT_EQ(framebuffer.count(Palette::Black), static_cast<size_t>(1280) * 720);
    EXPECT(framebuffer.guards_intact());
}

TEST_CASE(rendering_when_not_active_draws_nothing)
{
    TestFramebuffer framebuffer(1024, 768);
    State state; // Inactive.
    render_full(framebuffer.surface(), state);
    render_progress(framebuffer.surface(), state, 0);
    EXPECT_EQ(framebuffer.count(Palette::Black), static_cast<size_t>(1024) * 768);

    EXPECT(state.begin(1024, 768));
    EXPECT(state.end(EndReason::Handoff));
    render_full(framebuffer.surface(), state);
    EXPECT_EQ(framebuffer.count(Palette::Black), static_cast<size_t>(1024) * 768);
}

// ---- Missing/unusable surface fallback ----

TEST_CASE(unusable_surfaces_are_rejected_without_crashing)
{
    State state;
    EXPECT(state.begin(1024, 768));

    Surface null_surface { nullptr, 4096, 1024, 768 };
    EXPECT(!null_surface.is_usable());
    render_full(null_surface, state);
    render_progress(null_surface, state, 0);
    render_clear(null_surface, state);

    alignas(4) u8 buffer[64] = {};
    Surface zero_size { buffer, 64, 0, 0 };
    EXPECT(!zero_size.is_usable());
    render_full(zero_size, state);

    Surface pitch_too_small { buffer, 8, 1024, 768 };
    EXPECT(!pitch_too_small.is_usable());
    render_full(pitch_too_small, state);

    Surface misaligned_pitch { buffer, 4094, 1000, 4 };
    EXPECT(!misaligned_pitch.is_usable());
    render_full(misaligned_pitch, state);

    Surface misaligned_base { buffer + 1, 4096, 1, 1 };
    EXPECT(!misaligned_base.is_usable());
    render_full(misaligned_base, state);
}

TEST_CASE(surface_smaller_than_layout_never_writes_out_of_bounds)
{
    // A bad framebuffer description (layout computed for a bigger display than the real surface)
    // must clip, not corrupt memory.
    State state;
    EXPECT(state.begin(1920, 1080));
    TestFramebuffer framebuffer(300, 200);
    render_full(framebuffer.surface(), state);
    for (u8 index = 1; index < milestone_count; ++index) {
        u8 const previous = state.completed;
        state.advance(static_cast<Milestone>(index));
        render_progress(framebuffer.surface(), state, previous);
    }
    render_clear(framebuffer.surface(), state);
    EXPECT(framebuffer.guards_intact());
}

TEST_CASE(clipped_rendering_matches_the_visible_part_of_a_full_render)
{
    // Surfaces that cut straight through the splash's elements, both horizontally and vertically.
    // Drawing must clip at the edge: no wrap-around into the next row, nothing past the padding.
    State state;
    EXPECT(state.begin(1280, 720));
    for (u8 index = 1; index < milestone_count; ++index)
        state.advance(static_cast<Milestone>(index));
    auto const& layout = state.layout;

    TestFramebuffer full(1280, 720);
    render_full(full.surface(), state);

    DisplaySize const crops[] = {
        { layout.emblem.x + layout.emblem.width / 2, 720 },
        { layout.wordmark.x + layout.wordmark.width / 3, 720 },
        { 1280, layout.emblem.y + layout.emblem.height / 2 },
        { 1280, layout.progress.y + layout.progress.height / 2 },
        { layout.status.x + 7, layout.status.y + 5 },
    };
    for (auto const& crop : crops) {
        TestFramebuffer cropped(crop.width, crop.height);
        render_full(cropped.surface(), state);
        EXPECT(cropped.guards_intact());
        for (u32 y = 0; y < crop.height; ++y) {
            for (u32 x = 0; x < crop.width; ++x)
                EXPECT_EQ(cropped.at(x, y), full.at(x, y));
        }
        render_clear(cropped.surface(), state);
        EXPECT(cropped.guards_intact());
    }
}

TEST_CASE(layout_compiles_in_constant_expressions)
{
    // Layout math is constexpr, so it can be checked at compile time for any display we care about.
    static_assert(compute_layout(1920, 1080).valid);
    static_assert(compute_layout(1920, 1080).emblem.width == 24 * 9);
    static_assert(!compute_layout(0, 0).valid);
    static_assert(wordmark_characters_visible(milestone_count) == 8);
    static_assert(text_width(0, 5) == 0);
}
