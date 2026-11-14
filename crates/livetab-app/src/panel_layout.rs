/*
===============================================================================
Division By Zeros (DBZS) - LiveTabulator
Datei: panel_layout.rs
Zweck: Begrenzt vom UI angeforderte Panelmaße auf sichere, erreichbare Bereiche.
Sprache: Rust
===============================================================================
*/

/// Zweck: Begrenzt eine vom Splitter angeforderte Panelgröße auf sichere, erreichbare Pixelgrenzen.
/// Eingaben: `panel_id` – feste Kennung aus Slint; `requested_pixels` – angeforderte Größe in logischen Pixeln.
/// Ausgabe: Zulässige Größe in Pixeln oder `None`, falls die Panelkennung unbekannt ist.
pub(crate) fn clamp_panel_extent(panel_id: &str, requested_pixels: i32) -> Option<i32> {
    let (minimum, maximum) = match panel_id {
        "left" => (180, 648),
        "right" => (184, 648),
        "bottom" => (82, 360),
        _ => return None,
    };

    Some(requested_pixels.clamp(minimum, maximum))
}

/// Zweck: Berechnet den nächsten Sichtbarkeitszustand eines unterstützten Panels.
/// Eingaben: `panel_id` – `left`, `right` oder `bottom`; `currently_visible` – bisheriger UI-Zustand.
/// Ausgabe: Der umgeschaltete Sichtbarkeitszustand oder `None` bei unbekannter Panelkennung.
pub(crate) fn toggle_panel_visibility(panel_id: &str, currently_visible: bool) -> Option<bool> {
    match panel_id {
        "left" | "right" | "bottom" => Some(!currently_visible),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_panel_extent, toggle_panel_visibility};

    /// Zweck: Prüft, dass linke und rechte Spalten ihre Mindestbreite behalten.
    /// Eingaben: Extrem kleine Größenwünsche für beide Seitenpanels.
    /// Ausgabe: Keine; Assertions bestätigen die nicht unterschreitbaren Breiten.
    #[test]
    fn side_panels_keep_reachable_minimum_widths() {
        assert_eq!(clamp_panel_extent("left", i32::MIN), Some(180));
        assert_eq!(clamp_panel_extent("right", i32::MIN), Some(184));
    }

    /// Zweck: Prüft, dass Seitenpanels keine unkontrolliert großen Maße annehmen.
    /// Eingaben: Extrem große Größenwünsche für beide Seitenpanels.
    /// Ausgabe: Keine; Assertions bestätigen die festen oberen Sicherheitsgrenzen.
    #[test]
    fn side_panels_respect_upper_width_limits() {
        assert_eq!(clamp_panel_extent("left", i32::MAX), Some(648));
        assert_eq!(clamp_panel_extent("right", i32::MAX), Some(648));
    }

    /// Zweck: Prüft die Unter- und Obergrenzen der unteren Werkzeugleiste.
    /// Eingaben: Je ein zu kleiner und zu großer Höhenwunsch.
    /// Ausgabe: Keine; Assertions bestätigen Mindest- und Maximalhöhe.
    #[test]
    fn bottom_panel_respects_height_limits() {
        assert_eq!(clamp_panel_extent("bottom", 0), Some(82));
        assert_eq!(clamp_panel_extent("bottom", i32::MAX), Some(360));
    }

    /// Zweck: Prüft, dass gültige Größen erhalten und unbekannte Panels abgelehnt werden.
    /// Eingaben: Eine gültige Seitenbreite und eine nicht registrierte Panelkennung.
    /// Ausgabe: Keine; Assertions bestätigen unveränderte Werte und `None` bei unbekannter Kennung.
    #[test]
    fn valid_extent_is_preserved_and_unknown_panel_is_rejected() {
        assert_eq!(clamp_panel_extent("left", 340), Some(340));
        assert_eq!(clamp_panel_extent("unknown", 340), None);
    }

    /// Zweck: Prüft, dass alle Hauptpanels vom offenen in den geschlossenen Zustand und zurück wechseln.
    /// Eingaben: Die drei unterstützten Panelkennungen mit jeweils offenem und geschlossenem Ausgangszustand.
    /// Ausgabe: Keine; Assertions bestätigen Collapse, Restore und die Ablehnung einer unbekannten Kennung.
    #[test]
    fn panel_visibility_can_collapse_and_restore() {
        for panel_id in ["left", "right", "bottom"] {
            assert_eq!(toggle_panel_visibility(panel_id, true), Some(false));
            assert_eq!(toggle_panel_visibility(panel_id, false), Some(true));
        }
        assert_eq!(toggle_panel_visibility("unknown", true), None);
    }
}
