# Industrial Cyan GUI V2 — Integrationsnotiz

## Ziel

Die vorhandene Slint-Workstation erhält ein konsistentes, kontrastreiches Industrial-Cyan-Design. Der Rust-Core, die Worker, der Audiopfad und sämtliche Slint-/Rust-Callbacks bleiben unverändert.

## Änderungen

- Zentraler `IndustrialTheme` für Hintergrund, Panel, Linien, Signal- und Akzentfarben.
- Wiederverwendbarer `StatusChip`, direkt an `live_running` bzw. `runtime_loaded` gebunden.
- Schnellaktionen für Audiodateiauswahl und Live-Start/-Stopp über die bestehenden Backend-Callbacks.
- Separate Session-Statuszeile mit vorhandenen Projekt- und Laufzeitmeldungen.
- Kontrastreichere DSP-Kennzahlen, Waveform-Monitor, Timeline, Gitarrentabulatur und Lyrics-Bereich.
- Engere Seitenbereiche für mehr Platz in der mittleren Musikanalyse.
- Live-Monitor visualisiert ausschließlich vorhandenen Status und `waveform_text`; es werden keine synthetischen Audiosamples oder erfundenen CPU-/Latenzwerte angezeigt.

## Verträge

- Keine Änderung an Rust-Dateien oder Slint-Callback-Signaturen.
- Alle `in-out`-Properties und Backend-Integrationen bleiben erhalten.
- Die Schnellaktion „Live stoppen“ ist während einer laufenden WAV-Aufnahme deaktiviert, damit der bestehende Aufnahme-/Speicherworkflow beachtet wird.
- Die neue GUI ist ein Styling- und Layout-Slice, kein neuer Audio- oder KI-Backend-Stand.

## Verifikation

- `git diff --check` auf Whitespace-/Patchfehler.
- Automatischer Abgleich der Slint-Callback- und Property-Namen vor und nach der Umgestaltung.
- Ein nativer Slint-/Windows-Build muss auf einer Maschine mit Rust 1.92+ und den Projektruntimes ergänzt werden; die aktuelle Ausführungsumgebung enthält keine `cargo`-Toolchain.

## Folgeschritt

Die Text-basierte Waveform-/Tab-Visualisierung kann in einem getrennten Slice durch gemessene DSP-Samplemodelle und einen datengetriebenen Slint-Renderer ersetzt werden. Diese Änderung benötigt einen neuen Rust-UI-Datenvertrag und gehört daher bewusst nicht zum kompatiblen GUI-Slice.
