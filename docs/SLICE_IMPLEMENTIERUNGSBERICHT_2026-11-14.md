# LiveTabulator – Implementierungsbericht zum End-to-End-Slice

- **Datum:** 14. November 2026
- **Arbeitsstand:** Freigegebene Slice-Integration auf aktuellem `main` im Sandbox-Klon; der Windows-Arbeitsrechner blieb unberührt.
**Statussprache:** `CODE` bezeichnet implementierte Quellpfade; `AUTOMATED` gilt nur für tatsächlich ausgeführte Prüfungen; `NATIVE_E2E` verlangt einen echten Geräte-/GUI-Ablauf.

## Ergebnis

Der Slice verbindet native Slint-Oberfläche, Rust-Core, Live-Audio-/DSP-Transport, kanonisches Projektmodell, lokale Modelllaufzeit und atomische Persistenz. Der final integrierte Stand besteht den Linux-Sandbox-Workspace-Check, Debug-Build und Workspace-Tests; Formatprüfung und Clippy-Ergebnis sind unten ausdrücklich ausgewiesen. Ein Windows-Build des manuell auf den neueren `main` angepassten Stands sowie Geräte-/GUI-E2E sind nicht nachgewiesen.

## Implementierte Pfade

- **Kanonisches Modell und Timeline:** Schema 2 enthält stabile IDs, Event-Provenance sowie gemeinsame Tempo-/Takt- und Zeitumrechnung. Beim Öffnen erhalten Events ohne rekonstruierbare Herkunft explizit `Unknown`. Nested Lyrics-Wörter werden bei Migration, ID-Reservierung, Provenance und Validierung berücksichtigt; Top-Level-/Zeilenspiegel sind nur bei gleicher ID und gleichem Inhalt zulässig.
- **Rolling-Live-Beatgrid:** Die Controller-Übernahme aktualisiert das aktive Zeitfenster, erhält passende Event-IDs, schützt manuelle Beatkorrekturen und protokolliert Live-DSP-Herkunft einmalig. Ungültige Fenster werden vor der Projektänderung abgewiesen.
- **Live-Audio/DSP:** Capture-Start, Projektwechsel und Audioreferenzwechsel setzen Rolling- und Glättungszustände zurück. Die Übergabe nutzt einen vorallokierten Interleaved-SPSC-Ring; Echtzeittransport bleibt von DSP-, UI-, Job- und LLM-Worker getrennt. Aufnahme und WAV-/MP3-Writer erhalten die Quellkanäle; ASIO bleibt über einen begrenzten Kompatibilitätsworker an den Ring angebunden. Queue-/Drop-Metriken kommen aus dem Transport.
- **Musikalische Events und UI:** Pitch, Akkord, Noten, Beatgrid und Tonartkandidaten laufen über typisierte Events in die native Slint-Ansicht. Die kanonische Notenliste wird zeitlich sortiert und begrenzt projiziert; Undo/Redo, Live-Metriken und Seek sind an vorhandene Controller-Callbacks angebunden.
- **Korrektur und Lyrics:** Undo/Redo ist begrenzt; manuelle Korrekturen erhalten `UserEdit`. Das lokale Lyrics-Backend schreibt Wort-IDs und `LocalTranscription`-Provenance transaktional in das Projekt und aktualisiert den Playback-Scheduler.
- **Persistenz und Jobs:** Projekt-/Exportdateien werden über temporäre Geschwisterdateien erst nach vollständigem Schreiben ersetzt. Bestehende MP3- und Stem-Einträge in `.dbztab` bleiben erhalten. Jobverwaltung begrenzt parallele Jobs und Statushistorie.
- **Lokales Modell:** Qwen3.5-4B Q4_K_M bleibt der feste Assistant-Standard. Vor llama.cpp-Laden werden Manifest-ID, Dateiname, Größe und SHA-256 fail-closed geprüft; Inferenz läuft auf einem Hintergrundthread. Die Registry startet mit dem App-Modellordner und durchsucht `D:\Models` rekursiv; weitere Scans erfolgen nur auf Nutzeraktion.
- **Zusammenführung auf neueren `main`:** Aktuelle Änderungen am Audio-/Projektworkflow wurden beibehalten; ASIO ist durch die bounded-channel-Brücke an den neuen Ring angeschlossen. `Cargo.lock` und `crates/livetab-app/Cargo.toml` enthalten auf diesem `main` bereits den erforderlichen `serde_json`-Eintrag und haben deshalb keinen Inhaltsdiff; die tatsächlich geänderte Freigabemenge umfasst 22 statt 24 Pfade.

## Nicht als fertig ausgewiesen

- Im Repository ließ sich kein MIDI-Importpfad nachweisen; vorhanden sind MIDI-Export und separater BANDMAP-Import. Daher ist Schema-2-ID-/Provenance-Verhalten eines MIDI-Imports nicht belegt. MusicXML- und Guitar-Pro-Roundtrips sind ebenfalls nicht nachgewiesen.
- Der Windows-Quellpfad umfasst WASAPI-Systemmix-Loopback und die erhaltene ASIO-Anbindung; der ASIO-Pfad benötigt einen realen Windows-Treiber und wurde auf diesem Sandbox-Stand nicht ausgeführt. Mobile CPAL-Mikrofonpfade im Quelltext ersetzen keinen Android-/iOS-Crossbuild oder Gerätetest.
- Keine echte Audioquelle, Wiedergabe, Geräteunterbrechung, 60-Minuten-Stabilität, UI-DPI-/Fensterprobe, Latenz- oder Stressmessung wurde durchgeführt. Die ausgeführten Audiotests verwenden synthetische Daten bzw. Renderer ohne Gerät; es wurde keine Audioquelle oder Wiedergabe gestartet.
- Ein ausführbarer Release-/Installerpfad samt Runtime-DLLs, Lizenzinventar und Frischstartprobe ist nicht durch diesen Slice belegt. Drei ignorierte Tests benötigen lokale Qwen-Gewichte; reale Qwen-Lade-/Antwortqualität, lokale ASR-Modellverfügbarkeit und Stem-Separation sind nicht nachgewiesen.
- Score-/Measure-Virtualisierung sowie der umfassendere Studio-/Plugin-Backlog sind nicht als abgeschlossen zu verstehen.

## Validierung: tatsächliche Ergebnisse

Cargo lief offline auf Linux in der Sandbox mit Rust/Cargo 1.97.1 und dem vorhandenen Cargo-Cache. Es wurden keine Abhängigkeiten heruntergeladen und kein repo-weites Autoformat angewendet.

| Prüfung | Ergebnis |
|---|---|
| `cargo fmt --all -- --check` | **Nicht bestanden:** 189 Format-Diff-Blöcke im Workspace. Kein repo-weites Autoformat angewendet, um bestehende Nutzerformatierung nicht breit umzuschreiben. |
| `cargo check --workspace --all-targets --offline` | **Bestanden**; Dead-Code-Warnungen für `LiveNoteDraft.active`, plattformabhängige Capture-/Playback-Helfer. |
| `cargo build --workspace --all-targets --offline` | **Bestanden** im Linux-Sandboxprofil; mit denselben Dead-Code-Warnungen. |
| `cargo clippy --workspace --all-targets --offline` | **Exitcode 0**; Core: 28 Warnungen in der Library und 31 im Testtarget; App: 39 Warnungen in Binär- und Testtarget. Kein `clippy --fix` ausgeführt. |
| `cargo test --workspace --all-targets --offline` | **Bestanden:** Core 75 bestanden; App 23 bestanden, 3 ignoriert; 0 fehlgeschlagen. Die ignorierten App-Tests benötigen ausdrücklich ausgewählte lokale Qwen-GGUF-Dateien. |
| `cargo check -p livetab-app --all-targets --target x86_64-pc-windows-gnu --offline` | **Nicht abgeschlossen:** `llama-cpp-sys`-Bindgen konnte im Sandbox-Cross-Compiler `stdbool.h` nicht finden. Das ist eine fehlende MinGW-/Clang-Headerumgebung; es ist kein bestandener Windows-Build und belegt keinen Fehler im ASIO-Code. |
| `NATIVE_E2E` | Nicht durchgeführt; keine Windows-/Mobile-Hardware oder Audioquelle in der Sandbox verwendet. |

## Quellen und Entscheidungen

Maßgeblich waren `LIVETABULATOR_MASTERPLAN_2026-11.md` und `AGENTS.md`, ergänzt durch `LIVETABULATOR_IMPLEMENTIERUNGSPLAN_2026-11.md`, `docs/FEATURE_INVENTORY.md`, `docs/IMPLEMENTIERUNGSPLAN.md` und `docs/VISION.md`. Der konkrete Ist-Stand wurde in den Rust-/Slint-Dateien unter `crates/livetab-core` und `crates/livetab-app` geprüft. Ein unabhängiger Code-Review deckte die verschachtelten Lyrics-ID-Lücken und den ASIO-Starttimeout auf; beides wurde korrigiert und durch Core-Regressionstests beziehungsweise kontrollierten Worker-Join abgesichert. Fehlende Import-, Crossbuild-, Paket- oder Hardwarepfade werden nicht als fertig behauptet.
