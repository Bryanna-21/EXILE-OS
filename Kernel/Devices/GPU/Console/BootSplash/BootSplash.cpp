/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <Kernel/Devices/GPU/Console/BootSplash/BootSplash.h>

// NOTE: Everything in here runs in early boot. It must not allocate, take locks, call back into
//       other subsystems or VERIFY on anything a bad framebuffer description could influence.
//       All drawing is clamped to the surface, so a wrong layout can only draw less, never corrupt memory.

namespace Kernel::Graphics::BootSplash {

static void fill_rect(Surface const& surface, u64 x, u64 y, u64 width, u64 height, u32 color)
{
    if (x >= surface.width || y >= surface.height)
        return;
    u64 const x_end = x + width < surface.width ? x + width : surface.width;
    u64 const y_end = y + height < surface.height ? y + height : surface.height;

    for (u64 row = y; row < y_end; ++row) {
        auto* pixels = reinterpret_cast<u32*>(surface.data + row * surface.pitch);
        for (u64 column = x; column < x_end; ++column)
            pixels[column] = color;
    }
}

static void fill_rect(Surface const& surface, Rect const& rect, u32 color)
{
    fill_rect(surface, rect.x, rect.y, rect.width, rect.height, color);
}

static void draw_glyph(Surface const& surface, u8 const* rows, u64 x, u64 y, u32 scale, u32 color)
{
    if (!rows)
        return;
    for (u32 row = 0; row < glyph_rows; ++row) {
        for (u32 column = 0; column < glyph_columns; ++column) {
            if (rows[row] & (1u << (glyph_columns - 1 - column)))
                fill_rect(surface, x + static_cast<u64>(column) * scale, y + static_cast<u64>(row) * scale, scale, scale, color);
        }
    }
}

static void draw_text(Surface const& surface, StringView text, size_t first, size_t last, u64 x, u64 y, u32 scale, u32 color)
{
    for (size_t index = first; index < last && index < text.length(); ++index)
        draw_glyph(surface, glyph_for(text[index]), x + static_cast<u64>(index) * glyph_advance * scale, y, scale, color);
}

static void draw_emblem_parts(Surface const& surface, Layout const& layout, u8 from_part, u8 to_part)
{
    for (auto const& part : emblem_rects) {
        if (part.part < from_part || part.part >= to_part)
            continue;
        u32 const color = part.color == EmblemColor::Green ? Palette::Green : Palette::Blue;
        fill_rect(surface,
            static_cast<u64>(layout.emblem.x) + static_cast<u64>(part.x) * layout.cell,
            static_cast<u64>(layout.emblem.y) + static_cast<u64>(part.y) * layout.cell,
            static_cast<u64>(part.width) * layout.cell,
            static_cast<u64>(part.height) * layout.cell,
            color);
    }
}

static void draw_wordmark_characters(Surface const& surface, Layout const& layout, size_t from, size_t to)
{
    draw_text(surface, wordmark_text, from, to, layout.wordmark.x, layout.wordmark.y, layout.wordmark_scale, Palette::White);
}

static void draw_progress_segments(Surface const& surface, Layout const& layout, u8 from_completed, u8 to_completed)
{
    for (u8 segment = from_completed; segment < to_completed && segment < milestone_count; ++segment) {
        Rect const rect {
            layout.progress.x + segment * (layout.segment_width + layout.segment_gap),
            layout.progress.y,
            layout.segment_width,
            layout.progress.height,
        };
        fill_rect(surface, rect, Palette::Green);
    }
}

static void draw_progress_track(Surface const& surface, Layout const& layout)
{
    for (u8 segment = 0; segment < milestone_count; ++segment) {
        Rect const rect {
            layout.progress.x + segment * (layout.segment_width + layout.segment_gap),
            layout.progress.y,
            layout.segment_width,
            layout.progress.height,
        };
        fill_rect(surface, rect, Palette::DimBlue);
    }
}

static void draw_status(Surface const& surface, State const& state)
{
    auto const& layout = state.layout;
    // Replace the previous message. Only the status box is touched.
    fill_rect(surface, layout.status, Palette::Black);

    auto label = milestone_label(state.current_milestone());
    u32 const width = static_cast<u32>(text_width(label.length(), layout.status_scale));
    u32 const x = layout.status.x + (layout.status.width > width ? (layout.status.width - width) / 2 : 0);
    draw_text(surface, label, 0, label.length(), x, layout.status.y, layout.status_scale, Palette::Blue);
}

void render_full(Surface const& surface, State const& state)
{
    if (!surface.is_usable() || !state.is_active() || !state.layout.valid)
        return;
    auto const& layout = state.layout;

    draw_emblem_parts(surface, layout, 0, emblem_parts_visible(state.completed));
    draw_wordmark_characters(surface, layout, 0, wordmark_characters_visible(state.completed));
    draw_progress_track(surface, layout);
    draw_progress_segments(surface, layout, 0, state.completed);
    draw_status(surface, state);
}

void render_progress(Surface const& surface, State const& state, u8 previously_completed)
{
    if (!surface.is_usable() || !state.is_active() || !state.layout.valid)
        return;
    if (previously_completed >= state.completed)
        return;
    auto const& layout = state.layout;

    draw_emblem_parts(surface, layout, emblem_parts_visible(previously_completed), emblem_parts_visible(state.completed));
    draw_wordmark_characters(surface, layout, wordmark_characters_visible(previously_completed), wordmark_characters_visible(state.completed));
    draw_progress_segments(surface, layout, previously_completed, state.completed);
    draw_status(surface, state);
}

void render_clear(Surface const& surface, State const& state)
{
    if (!surface.is_usable() || !state.layout.valid)
        return;
    fill_rect(surface, state.layout.bounds, Palette::Black);
}

}
