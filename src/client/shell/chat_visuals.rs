//! collie: Farben und Mini-Grafiken der Chat-Karten. Alles hier sind reine
//! Funktionen über Snapshot-Werte und die Uhrzeit; nichts liest Bildschirm,
//! Dateien oder Prozesse.

use ratatui::style::Color;

use super::Palette;

/// So lange leuchtet eine Karte nach, wenn ihr Chat neu auf den Nutzer wartet.
pub(super) const CARD_PULSE_MS: u64 = 1_500;
/// Dauer eines Frames der schlafenden Collie.
pub(super) const DOZE_FRAME_MS: u64 = 900;
/// Spalten der Aktivitätskurve auf einer Karte.
pub(super) const SPARK_BUCKETS: usize = 12;
/// Zellen des Kontext-Balkens.
pub(super) const GAUGE_CELLS: usize = 5;

const SPARK_LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Mischt `from` und `to`; `amount` 0 ist `from`, 1 ist `to`. Ohne RGB-Werte,
/// etwa beim Terminal-Theme, springt die Farbe in der Mitte um.
pub(super) fn blend(from: Color, to: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |a: u8, b: u8| {
                (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8
            };
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ if amount < 0.5 => from,
        _ => to,
    }
}

/// Fester Farbton eines Repos. Rot und Gelb bleiben den Status-Farben.
pub(super) fn repo_color(repo: &str, palette: &Palette) -> Color {
    let hues = [
        palette.blue,
        palette.mauve,
        palette.green,
        palette.teal,
        palette.peach,
        blend(palette.mauve, palette.red, 0.5),
        blend(palette.blue, palette.teal, 0.5),
        blend(palette.green, palette.teal, 0.5),
    ];
    // FNV-1a: stabil über Läufe und Rechner, anders als der Std-Hasher.
    let hash = repo.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    hues[(hash % hues.len() as u64) as usize]
}

/// Kürzel eines Repos: Anfangsbuchstaben der Wortteile, sonst die ersten
/// zwei Zeichen. `second-brain` wird `SB`, `collie` wird `CO`.
pub(super) fn repo_initials(repo: &str) -> String {
    let parts = repo
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let initials: String = if parts.len() >= 2 {
        parts
            .iter()
            .filter_map(|part| part.chars().next())
            .take(2)
            .collect()
    } else {
        parts
            .first()
            .map_or_else(String::new, |part| part.chars().take(2).collect())
    };
    initials.to_uppercase()
}

/// Farbe des Wärmestreifens: grau, nach 15 Minuten gelb, nach einer Stunde
/// peach, nach vier Stunden rot, dazwischen fließend.
pub(super) fn heat_color(since: u64, now: u64, palette: &Palette) -> Color {
    if since == 0 {
        return palette.surface1;
    }
    let minutes = now.saturating_sub(since) as f32 / 60_000.0;
    let stops = [
        (0.0, palette.overlay0),
        (15.0, palette.yellow),
        (60.0, palette.peach),
        (240.0, palette.red),
    ];
    for pair in stops.windows(2) {
        let ((start, from), (end, to)) = (pair[0], pair[1]);
        if minutes < end {
            return blend(from, to, (minutes - start) / (end - start));
        }
    }
    palette.red
}

/// Stärke des Nachleuchtens: 1 direkt nach dem Wechsel, dann abklingend bis 0.
/// Eine Zeit in der Zukunft, etwa durch eine abweichende Server-Uhr, leuchtet
/// nicht.
pub(super) fn pulse_amount(since: u64, now: u64) -> f32 {
    if since == 0 || now < since {
        return 0.0;
    }
    let elapsed = now - since;
    if elapsed >= CARD_PULSE_MS {
        return 0.0;
    }
    let remaining = 1.0 - elapsed as f32 / CARD_PULSE_MS as f32;
    remaining * remaining
}

/// Ob gerade eine Karte nachleuchtet und deshalb neu gezeichnet werden muss.
pub(super) fn pulsing(since: Option<u64>, now: u64) -> bool {
    since.is_some_and(|since| pulse_amount(since, now) > 0.0)
}

/// Belegung des Kontexts zwischen 0 und 1, wenn Tokens und Fenster bekannt
/// sind.
pub(super) fn context_ratio(tokens: Option<u64>, window: Option<u64>) -> Option<f32> {
    let window = window.filter(|window| *window > 0)?;
    Some((tokens? as f32 / window as f32).clamp(0.0, 1.0))
}

/// Balken aus [`GAUGE_CELLS`] Zellen, etwa `▰▰▰▱▱`.
pub(super) fn context_gauge(ratio: f32) -> String {
    let filled = (ratio * GAUGE_CELLS as f32).round() as usize;
    let filled = filled.min(GAUGE_CELLS);
    "▰".repeat(filled) + &"▱".repeat(GAUGE_CELLS - filled)
}

/// Farbe des Kontext-Balkens: grün, ab 70 % gelb, ab 90 % peach.
pub(super) fn context_color(ratio: f32, palette: &Palette) -> Color {
    if ratio >= 0.9 {
        palette.peach
    } else if ratio >= 0.7 {
        palette.yellow
    } else {
        palette.green
    }
}

/// Anteil Arbeitszeit je Spalte über die letzten 30 Minuten, älteste zuerst.
/// Eine laufende Phase zählt bis `now`.
pub(super) fn activity_buckets(
    spans: &[(u64, u64)],
    working_since: Option<u64>,
    now: u64,
) -> [f32; SPARK_BUCKETS] {
    let window = crate::workspace::WORKING_SPAN_WINDOW_MS;
    let bucket = window / SPARK_BUCKETS as u64;
    let start = now.saturating_sub(window);
    let mut buckets = [0.0; SPARK_BUCKETS];
    let open = working_since.map(|since| (since, now));
    for (span_start, span_end) in spans.iter().copied().chain(open) {
        let span_start = span_start.max(start);
        let span_end = span_end.min(now);
        if span_end <= span_start {
            continue;
        }
        for (index, value) in buckets.iter_mut().enumerate() {
            let bucket_start = start + index as u64 * bucket;
            let bucket_end = bucket_start + bucket;
            let overlap = span_end
                .min(bucket_end)
                .saturating_sub(span_start.max(bucket_start));
            *value += overlap as f32 / bucket as f32;
        }
    }
    buckets.map(|value| value.min(1.0))
}

/// Ein Zeichen der Aktivitätskurve; `None` für eine Spalte ohne Arbeit.
pub(super) fn spark_symbol(value: f32) -> Option<&'static str> {
    if value <= 0.0 {
        return None;
    }
    let level = (value * SPARK_LEVELS.len() as f32).ceil() as usize;
    Some(SPARK_LEVELS[level.clamp(1, SPARK_LEVELS.len()) - 1])
}

/// Schlafende Collie für den leeren Zustand.
pub(super) const COLLIE: [&str; 5] = [
    "   / \\__",
    "  (    -\\___",
    "  /         O",
    " /   (_____/",
    "/_____/   U",
];
/// Breite der Collie in Zellen.
pub(super) const COLLIE_WIDTH: u16 = 13;

/// Die zwei Zeilen über dem Kopf: aufsteigende `z` je nach Frame.
pub(super) fn doze_frame(now: u64) -> [&'static str; 2] {
    match (now / DOZE_FRAME_MS) % 3 {
        0 => ["", "         z"],
        1 => ["           z", "         z"],
        _ => ["           Z", ""],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_mixes_rgb_and_snaps_named_colors() {
        assert_eq!(
            blend(Color::Rgb(0, 0, 0), Color::Rgb(200, 100, 50), 0.5),
            Color::Rgb(100, 50, 25)
        );
        assert_eq!(blend(Color::Blue, Color::Red, 0.2), Color::Blue);
        assert_eq!(blend(Color::Blue, Color::Red, 0.8), Color::Red);
    }

    #[test]
    fn repo_color_is_stable_and_avoids_status_colors() {
        let palette = Palette::catppuccin();
        assert_eq!(
            repo_color("collie", &palette),
            repo_color("collie", &palette)
        );
        for repo in ["collie", "second-brain", "herdr", "dotfiles", "a", ""] {
            let color = repo_color(repo, &palette);
            assert_ne!(color, palette.red);
            assert_ne!(color, palette.yellow);
        }
    }

    #[test]
    fn repo_initials_use_word_parts() {
        assert_eq!(repo_initials("collie"), "CO");
        assert_eq!(repo_initials("second-brain"), "SB");
        assert_eq!(repo_initials("my_cool.repo"), "MC");
        assert_eq!(repo_initials("x"), "X");
        assert_eq!(repo_initials("--"), "");
    }

    #[test]
    fn heat_runs_from_grey_to_red() {
        let palette = Palette::catppuccin();
        let now = 10 * 3_600_000;
        assert_eq!(heat_color(now, now, &palette), palette.overlay0);
        assert_eq!(heat_color(now - 15 * 60_000, now, &palette), palette.yellow);
        assert_eq!(heat_color(now - 60 * 60_000, now, &palette), palette.peach);
        assert_eq!(heat_color(now - 5 * 3_600_000, now, &palette), palette.red);
        assert_eq!(heat_color(0, now, &palette), palette.surface1);
        let between = heat_color(now - 7 * 60_000, now, &palette);
        assert_ne!(between, palette.overlay0);
        assert_ne!(between, palette.yellow);
    }

    #[test]
    fn pulse_fades_and_ignores_future_times() {
        let since = 1_000_000;
        assert_eq!(pulse_amount(since, since), 1.0);
        assert!(pulse_amount(since, since + 500) < 1.0);
        assert_eq!(pulse_amount(since, since + CARD_PULSE_MS), 0.0);
        assert_eq!(pulse_amount(since, since - 1), 0.0);
        assert_eq!(pulse_amount(0, since), 0.0);
    }

    #[test]
    fn context_gauge_rounds_cells() {
        assert_eq!(context_ratio(Some(50), Some(100)), Some(0.5));
        assert_eq!(context_ratio(Some(50), None), None);
        assert_eq!(context_ratio(Some(50), Some(0)), None);
        assert_eq!(context_gauge(0.0), "▱▱▱▱▱");
        assert_eq!(context_gauge(0.64), "▰▰▰▱▱");
        assert_eq!(context_gauge(1.0), "▰▰▰▰▰");
    }

    #[test]
    fn activity_buckets_split_spans_over_the_window() {
        let now = 100 * 60_000;
        let bucket = crate::workspace::WORKING_SPAN_WINDOW_MS / SPARK_BUCKETS as u64;
        // Die letzte Spalte ist voll, die vorletzte halb.
        let spans = [(now - bucket - bucket / 2, now - bucket)];
        let buckets = activity_buckets(&spans, Some(now - bucket), now);
        assert_eq!(buckets[SPARK_BUCKETS - 1], 1.0);
        assert_eq!(buckets[SPARK_BUCKETS - 2], 0.5);
        assert_eq!(buckets[0], 0.0);
        // Phasen vor dem Fenster zählen nicht.
        let old = activity_buckets(&[(0, 10_000)], None, now);
        assert!(old.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn spark_symbol_maps_levels() {
        assert_eq!(spark_symbol(0.0), None);
        assert_eq!(spark_symbol(0.01), Some("▁"));
        assert_eq!(spark_symbol(0.5), Some("▄"));
        assert_eq!(spark_symbol(1.0), Some("█"));
    }

    #[test]
    fn collie_fits_its_width() {
        for line in COLLIE {
            assert!(line.chars().count() <= usize::from(COLLIE_WIDTH));
        }
        for frame in 0..3 {
            for line in doze_frame(frame * DOZE_FRAME_MS) {
                assert!(line.chars().count() <= usize::from(COLLIE_WIDTH));
            }
        }
    }
}
