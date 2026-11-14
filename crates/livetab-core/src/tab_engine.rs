/*
===============================================================================
Division By Zeros (DBZS) - Rust-Tab-Engine
Modul: tab_engine
Zweck: MIDI-/Melodienoten auf sechssaitige Gitarrenstimmungen abbilden,
       Griffbewegungen optimieren und lesbare ASCII-Tabs erzeugen.
Sprache: Rust
Hinweise:
- Der Solver bewertet Saite, Bund, offene Saiten, Lage und vorherige Griffposition.
- Nicht spielbare Noten werden als Fehler gemeldet, nicht still verworfen.
===============================================================================
*/
use crate::domain::TimeSignature;
use crate::project_model::{FrettedNote, GuitarTuning, TabTrack};
use crate::song::{NoteEvent, TabNote};

const MAX_FRET: u8 = 24;

#[derive(Debug, Clone, Copy)]
struct Candidate {
    string_index_low_to_high: usize,
    fret: u8,
    local_cost: f64,
}

/// Zweck: Erzeugt eine Gitarrentabspur aus MIDI-Noten und einem validierten Gitarrentuning.
/// Eingaben: Notenereignisse, Stimmung einschließlich Capo und Spurbezeichnung.
/// Ausgabe: Zeitlich sortierte `TabTrack`-Noten oder Fehler für ungültige Stimmung/unspielbare Töne.
pub fn generate_tab_track(
    notes: &[NoteEvent],
    tuning: GuitarTuning,
    name: impl Into<String>,
) -> Result<TabTrack, String> {
    tuning.validate()?;
    if notes.is_empty() {
        return Ok(TabTrack {
            id: crate::song::next_event_id(),
            name: name.into(),
            tuning,
            notes: Vec::new(),
            generated: true,
        });
    }
    let mut ordered = notes.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
    let mut candidate_layers = Vec::with_capacity(ordered.len());
    for note in &ordered {
        let mut candidates = Vec::new();
        for (string_index, open) in tuning.open_midi_low_to_high.iter().enumerate() {
            let open_pitch = u16::from(*open) + u16::from(tuning.capo);
            if u16::from(note.midi()) >= open_pitch {
                let fret = u16::from(note.midi()) - open_pitch;
                if fret <= u16::from(MAX_FRET) {
                    let fret = fret as u8;
                    let open_string_bonus = if fret == 0 { -0.7 } else { 0.0 };
                    let high_fret_penalty = f64::from(fret.saturating_sub(12)) * 0.35;
                    let local_cost = f64::from(fret) * 0.08 + high_fret_penalty + open_string_bonus;
                    candidates.push(Candidate {
                        string_index_low_to_high: string_index,
                        fret,
                        local_cost,
                    });
                }
            }
        }
        if candidates.is_empty() {
            return Err(format!(
                "{} ({}) ist auf keiner Saite bis Bund {MAX_FRET} spielbar.",
                note.pitch(),
                note.start_seconds()
            ));
        }
        candidate_layers.push(candidates);
    }
    let mut costs: Vec<Vec<f64>> = Vec::with_capacity(ordered.len());
    let mut predecessors: Vec<Vec<usize>> = Vec::with_capacity(ordered.len());
    costs.push(
        candidate_layers[0]
            .iter()
            .map(|candidate| candidate.local_cost)
            .collect(),
    );
    predecessors.push(vec![0; candidate_layers[0].len()]);
    for layer in 1..candidate_layers.len() {
        let previous_layer = &candidate_layers[layer - 1];
        let current_layer = &candidate_layers[layer];
        let mut current_costs = Vec::with_capacity(current_layer.len());
        let mut current_predecessors = Vec::with_capacity(current_layer.len());
        for candidate in current_layer {
            let (best_index, best_cost) = previous_layer
                .iter()
                .enumerate()
                .map(|(index, previous)| {
                    let string_jump = previous
                        .string_index_low_to_high
                        .abs_diff(candidate.string_index_low_to_high)
                        as f64
                        * 1.35;
                    let fret_jump = previous.fret.abs_diff(candidate.fret) as f64 * 0.18;
                    let position_change = if previous.fret.abs_diff(candidate.fret) > 5 {
                        1.0
                    } else {
                        0.0
                    };
                    (
                        index,
                        costs[layer - 1][index]
                            + string_jump
                            + fret_jump
                            + position_change
                            + candidate.local_cost,
                    )
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
                .unwrap();
            current_costs.push(best_cost);
            current_predecessors.push(best_index);
        }
        costs.push(current_costs);
        predecessors.push(current_predecessors);
    }
    let last_layer = costs.len() - 1;
    let mut choice = costs[last_layer]
        .iter()
        .enumerate()
        .min_by(|left, right| left.1.total_cmp(right.1))
        .map(|(index, _)| index)
        .unwrap();
    let mut selected = vec![0usize; ordered.len()];
    selected[last_layer] = choice;
    for layer in (1..=last_layer).rev() {
        choice = predecessors[layer][choice];
        selected[layer - 1] = choice;
    }
    let mut fretted = Vec::with_capacity(ordered.len());
    for (index, note) in ordered.iter().enumerate() {
        let candidate = candidate_layers[index][selected[index]];
        let string_number = (6 - candidate.string_index_low_to_high) as u8;
        let position = TabNote::new(
            string_number,
            candidate.fret,
            note.start_seconds(),
            note.duration_seconds(),
        )
        .map_err(|error| error.to_string())?;
        fretted.push(FrettedNote {
            midi: note.midi(),
            position,
        });
    }
    Ok(TabTrack {
        id: crate::song::next_event_id(),
        name: name.into(),
        tuning,
        notes: fretted,
        generated: true,
    })
}

/// Zweck: Rendert eine Tabspur zeilenweise als lesbare sechssaitige ASCII-Tabulatur.
/// Eingaben: Tabspur, BPM, Taktart und optionale maximale Zahl dargestellter Ereignisse.
/// Ausgabe: Text mit Stimmung, Standard-Tuning, Taktnummern und sechs Gitarrensaiten.
pub fn format_ascii_tab(
    track: &TabTrack,
    bpm: f64,
    time_signature: TimeSignature,
    maximum_notes: usize,
) -> String {
    if track.notes.is_empty() {
        return format!("{} · {} · keine Tab-Noten", track.name, track.tuning.name);
    }
    if !bpm.is_finite() || bpm <= 0.0 {
        return "Ungültiges Tempo: Tabulatur kann nicht nach Takten gruppiert werden.".into();
    }
    let mut notes = track
        .notes
        .iter()
        .take(maximum_notes.max(1))
        .collect::<Vec<_>>();
    notes.sort_by(|left, right| {
        left.position
            .start_seconds()
            .total_cmp(&right.position.start_seconds())
    });
    let mut bars: Vec<Vec<&FrettedNote>> = Vec::new();
    let mut current_bar = 0u64;
    for note in notes {
        let Ok(position) =
            crate::domain::MusicalPosition::at(note.position.start_seconds(), bpm, time_signature)
        else {
            continue;
        };
        if bars.is_empty() || position.bar != current_bar {
            current_bar = position.bar;
            bars.push(Vec::new());
        }
        bars.last_mut().unwrap().push(note);
    }
    let mut output = format!(
        "{} · Stimmung {} · Capo {}\n",
        track.name, track.tuning.name, track.tuning.capo
    );
    for string_number in (1u8..=6).rev() {
        let label = match string_number {
            1 => "e",
            2 => "B",
            3 => "G",
            4 => "D",
            5 => "A",
            _ => "E",
        };
        output.push_str(&format!("{label}|"));
        for bar in &bars {
            let mut cells = Vec::new();
            for note in bar
                .iter()
                .filter(|item| item.position.string_number() == string_number)
            {
                cells.push(format!("{:02}", note.position.fret()));
            }
            if cells.is_empty() {
                output.push_str("----|");
            } else {
                for (index, cell) in cells.iter().enumerate() {
                    if index > 0 {
                        output.push('-');
                    }
                    output.push_str(cell);
                }
                output.push('|');
            }
        }
        output.push('\n');
    }
    output
}

/// Zweck: Liefert die Standardstimmung als benutzerfreundliche Notenbezeichnung.
/// Eingaben: `tuning` – gültige sechssaitige MIDI-Stimmung.
/// Ausgabe: Tiefe bis hohe Saite, zum Beispiel `E2 A2 D3 G3 B3 E4`.
pub fn describe_tuning(tuning: &GuitarTuning) -> String {
    tuning
        .open_midi_low_to_high
        .iter()
        .map(|midi| {
            let name = crate::dsp::midi_note_name(*midi);
            name
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::NoteEvent;

    /// Zweck: Prüft die Standardstimmung und den globalen Fingering-Solver an bekannten MIDI-Tönen.
    /// Eingaben: Keine; spielt E4, F4 und G4 in EADGBE.
    /// Ausgabe: Keine Rückgabe; Assertions sichern offene hohe E-Saite und spielbare Bundwerte.
    #[test]
    fn maps_melody_to_playable_standard_tuning() {
        let notes = [
            NoteEvent::new(64, 0.0, 0.4).unwrap(),
            NoteEvent::new(65, 0.5, 0.4).unwrap(),
            NoteEvent::new(67, 1.0, 0.4).unwrap(),
        ];
        let track = generate_tab_track(&notes, GuitarTuning::standard(), "Lead").unwrap();
        assert_eq!(track.notes[0].position.string_number(), 1);
        assert_eq!(track.notes[0].position.fret(), 0);
        assert!(track.notes.iter().all(|note| note.position.fret() <= 24));
        assert!(format_ascii_tab(&track, 120.0, TimeSignature::default(), 32).contains("e|"));
    }

    /// Zweck: Meldet fehlende Noten als leere Tabspur und nicht spielbare tiefe Töne klar zurück.
    /// Eingaben: Keine; prüft leere Eingabe und MIDI 0 mit Standardstimmung.
    /// Ausgabe: Keine Rückgabe; Assertions sichern leeren Erfolg und erklärenden Fehler.
    #[test]
    fn handles_empty_and_unplayable_notes_explicitly() {
        assert!(
            generate_tab_track(&[], GuitarTuning::standard(), "Lead")
                .unwrap()
                .notes
                .is_empty()
        );
        let impossible = [NoteEvent::new(0, 0.0, 0.2).unwrap()];
        assert!(
            generate_tab_track(&impossible, GuitarTuning::standard(), "Lead")
                .unwrap_err()
                .contains("keiner Saite")
        );
    }
}
