# Rust/Slint-Neuaufbau — Umsetzungsstatus

Die ursprünglichen Projektpapiere und das UI-Konzept bleiben unverändert erhalten. Der neue Umsetzungsstand und abweichende technische Entscheidungen werden hier getrennt dokumentiert.

## Architekturentscheidungen

- Rust/Slint-App und Rust-Core statt C#/Avalonia; Python ist nicht der zeitkritische Kern.
- Die Audioanalyse soll als eigener Worker/Thread laufen. llama.cpp-Inferenz darf nie den Audio-Callback blockieren. Die Runtime liegt bereits auf einem separaten Worker; Audioaufnahme und Live-Audiopipeline sind noch nicht implementiert.
- llama.cpp wird über ein gekapseltes Rust-Adaptermodul eingebunden. Der Windows-Prototyp nutzt `llama-cpp-4` 0.11.0 mit dynamischem CPU/OpenMP-Link und optionalem CUDA-Profil. Windows-CUDA wurde auf der GTX 1650 SUPER gebaut und gemessen. Für den nativen Windows-Build ist Ninja erforderlich, weil MSBuild das von der Build-Abhängigkeit übergebene `-j12` mit `MSB1001` zurückweist. CUDA-/llama.cpp-DLLs sind noch nicht in ein Releasepaket integriert; Metal/Vulkan und Android-/iOS-Builds sind offen.
- Das feste Assistant-Standardmodell ist Qwen3.5-4B Agentic Coder v4 Q4_K_M (2.783.447.520 Bytes; Apache-2.0) unter `crates/livetab-app/models/llm/Qwen3.5-4B-Q4_K_M.gguf`. SHA-256: `C1268EF0EC3B0316EA62C081645D7BBAF80CF76288E4F6A590C22A073D64C920`. Das zuvor app-lokale Qwen3.5-0.8B-Q4_0-Testmodell wurde nach erfolgreicher 4B-CPU-/CUDA-Prüfung entfernt. Modellgewichte bleiben von Git ausgeschlossen; Mobile-Apps benötigen später eine passende App-Speicher-/Downloadstrategie.
- Die Registry validiert GGUF-Metadaten vor der Auswahl. Ein beliebiges `.gguf`-Dateiformat (z. B. ein VAD-Modell) ist nicht automatisch ein LLM.
- Chord-/Tonartberechnung ist ein eigener Rust-DSP-/Musiktheoriekern; llama.cpp dient nur für nicht zeitkritische Erklärung und Assistenz.
- Die Rust-API behält internationale Notennamen bei und ergänzt deutsche Darstellung: `NoteName::H` ist ein Alias der internationalen Tonklasse B; `as_german_str()` liefert B für den Halbton 10 und H für den Halbton 11; `Chord::german_name()` formatiert Akkorde deutsch.

## C#-Domänenmodell-Parität

- Die Legacy-Aliase `Db`, `Eb`, `Gb`, `Ab` und `Bb` sind als Rust-Konstanten verfügbar; serialisiert werden weiterhin die kanonischen Kreuz-Varianten. `Chord::midi_notes_for_base_octave(base_octave)` bildet die C#-Formel ab; `TimeSignature::with_default_denominator(numerator)` und `beat_value()` bilden Standardnenner und `BeatValue` ab.
- `NoteEvent::with_pitch_label()` bewahrt importierte Schreibweisen wie `Bb4`; ohne Label wird eine kanonische Schreibweise abgeleitet. Der gespeicherte String macht `NoteEvent` `Clone`, nicht `Copy`.
- Bewusste Grenzen: Chord-/Lyrics-Positionen werden in Rust mit übergebenem BPM und Taktart berechnet; die alte fest verdrahtete Lyrics-Beat-Näherung wird nicht übernommen. Rust validiert Zeit-, MIDI- und Taktwerte strenger, kapselt mutierbare Felder, speichert Song-Zeitpunkte als Unix-Millisekunden und verwendet `f32` statt C#-`double` für Konfidenzen.

## Status

- [x] Recherche zu Plattformgrenzen, Slint, Modell-/Runtime-Lizenzen und lokalen Buildwerkzeugen dokumentiert.
- [x] Workspace- und Core-Grundgerüst angelegt.
- [x] Rekursive Modell-Registry mit `D:\Models` als Windows-Standardwurzel und `LIVETAB_MODEL_PATHS` als Override implementiert.
- [x] GGUF-Header/Metadaten fail-closed geprüft; Textarchitekturen von VAD/ASR und unbekannten Architekturen getrennt.
- [x] Core-Template-Akkord- und Krumhansl-Schmuckler-Tonartenschätzung implementiert.
- [x] C#-Domänenreferenz ausschließlich gelesen; neue Rust-Typen für Noten, Akkorde, Tonarten, Taktarten, Songs, Akkord-/Noten-/Lyrics-/Tab-Ereignisse angelegt und im Core exportiert.
- [x] Deutsche B/H-Notation in Rust ergänzt und mit Tests abgesichert; die internationale Ausgabe bleibt rückwärtskompatibel.
- [x] Slint-Prototyp mit Registry-, Lade-, Inferenz- und Projektimport-Callbacks; alle 180 Rust-Funktionsdefinitionen sowie Slint-/Rust-UI-Callbacks sind mit deutschem Zweck, Eingaben und Ausgabe dokumentiert; Kommentar-Audit bestanden.
- [x] Qwen3.5-4B Q4_K_M in den App-Modellordner kopiert, bytegenaue SHA-256 bestätigt, als festes Registry-Standardmodell verdrahtet und im Manifest markiert.
- [x] Temporäres app-lokales 0.8B-Modell nach erfolgreicher 4B-CPU-/CUDA-Verifikation entfernt; die externe `D:\Models`-Ablage wurde nicht verändert.
- [x] Windows llama.cpp CPU- und CUDA-Profile haben das feste 4B-GGUF geladen und Tokens erzeugt.
- [x] Slint-Oberfläche an Registry und nicht-blockierenden Runtime-Worker angebunden; der Windows-Workspace kompiliert.
- [x] `.bandmap`-Import für V1/V2-Recovery- und V3-Portable-Container im neuen Rust-Core ergänzt; ZIP-Pfade, Größen, Manifeste und SQLite-Kopf werden vor der sicheren Extraktion geprüft.
- [x] Projektpersistenz unter `%LOCALAPPDATA%\DBZS\LiveTabulator` ergänzt: Originaleinträge unter `data/`, Metadaten in `project.json`, Wiederherstellung über `last-project`, identische Archive werden per SHA-256 wiederverwendet.
- [x] Slint-Pfadfeld, nativen Windows-Dateidialog mit `.bandmap`-Filter und gemeinsamen nicht-blockierenden Importworker mit Neustartwiederherstellung angebunden; Android-/iOS-Verfügbarkeit ist nicht getestet.
- [ ] VRAM automatisch ermitteln und GPU-Layerlast daran ausrichten; derzeit bleibt der CUDA-Standard vorsichtig bei 8 Layern, Messprofile werden explizit über `LIVETAB_GPU_LAYERS` gesetzt.
- [ ] Releasepaket mit passender Runtime-/CUDA-DLL-Auswahl und SHA-256-Prüfung für das Modell erzeugen.
- [ ] Android-/iOS-Builds auf geeigneter Toolchain und echten Zielgeräten prüfen.
- [x] Windows-WASAPI-Loopback-Capture, lokaler Live-DSP sowie automatische lokale Rust-DSP-Analyse und Vier-Stem-Trennung gespeicherter Liveaufnahmen; fehlende Stem-Modelle erfordern eine ausdrückliche Downloadfreigabe.
- [ ] Live-Pitch-/Beat-Ausbau und Control Desk implementieren; echte WASAPI-Geräte-/GUI-Laufzeit und Audio-Latenz sind noch nicht gemessen.
- [x] Separaten ASIO-Interfaceeingang über die MIT-lizenzierte Rust-Crate `azo` integrieren; getrennt von WASAPI-Systemmix. Vollständiger Windows-x64-GNU-Appbuild (`cargo check` und `cargo build`) wurde im Linux-Sandbox-Crossbuild erfolgreich verifiziert; reale Treiber-/Geräteprüfung bleibt offen. Details: `WINDOWS_CROSS_BUILD_STATUS.md`.

## Quellen

- llama.cpp / Android: https://github.com/ggml-org/llama.cpp/blob/master/README.md und https://github.com/ggml-org/llama.cpp/blob/master/docs/android.md
- llama.cpp Buildoptionen: https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md
- Rust-Bindings: https://crates.io/crates/llama-cpp-4 und https://docs.rs/llama-cpp-4/latest/llama_cpp_4/
- Festes Qwen3.5-4B-Q4_K_M-GGUF und Apache-2.0-Lizenz: https://huggingface.co/mradermacher/qwen3.5-4b-agentic-coder-v4-GGUF; Quell-Finetune: https://huggingface.co/mahiatlinux/qwen3.5-4b-agentic-coder-v4
- Slint Android/iOS/Windows-Planung: siehe `RUST_REBUILD_RESEARCH.md`.
- Nativer Dateidialog: `rfd` 0.16.0, lokal geprüfte `FileDialog`-API; Paketquelle und Version: https://crates.io/crates/rfd/0.16.0.

## BANDMAP-Import und Projektpersistenz — Stand 2026-11-08

- Neu in Rust: Import von V1/V2-Recovery-Containern (`manifest.properties`, `project.properties`) und V3-Portable-Containern (`manifest.json`, `project.sqlite`); alle validierten ZIP-Einträge und Medien bleiben im Projektordner erhalten.
- `ProjectStore` schreibt unter `%LOCALAPPDATA%\DBZS\LiveTabulator\projects`; SHA-256-basierte Kennung, Staging-Ordner mit anschließendem atomarem Umbenennen, `project.json` und `last-project` sichern Duplikatvermeidung und Wiederherstellung.
- Zusätzlich zum manuellen Pfadfeld bietet die Slint-UI im Windows-Build einen nativen `rfd`-Dialog mit `.bandmap`-Filter. Beide Wege zeigen den Pfad an und verwenden denselben Worker: Der Core prüft Endung und V1/V2/V3-Container, persistiert über `ProjectStore` und meldet Erfolg mit Version/Titel/Speicherort oder den Fehler; Dialogabbruch wird ebenfalls angezeigt.
- Der Picker ist in diesem Slice bewusst nur für Windows eingebunden. Andere Plattform-Builds kennzeichnen ihn als nicht verfügbar und behalten den manuellen Pfadimport; Android-/iOS-Builds und tatsächliche Picker-Verfügbarkeit dort wurden nicht getestet.
- Das app-lokale `crates/livetab-app/models/llm/Qwen3.5-4B-Q4_K_M.gguf` ist vorhanden (2.783.447.520 Bytes). `models/manifest.json` markiert ID `qwen3_5_4b_q4_k_m` als Default und verweist mit `llm/Qwen3.5-4B-Q4_K_M.gguf` auf diese Datei; der gezielte Registry-Test bestätigt Pfadwurzel, Größe und Architektur `qwen35`. Dafür wurde nur der App-Modellordner geprüft, kein `D:\Models`-Scan ausgeführt und das Modell nicht verändert.
- Cargo-Manifest/Lockfile ergänzen ZIP-Import und SHA-256; die nötigen Crates wurden einmalig über den lokalen Relay geladen, danach liefen die Prüfungen mit `--locked --offline`.

### Aktuelle Testergebnisse auf RAZOR

- `cargo test -p livetab-core --config .work/tools/cargo-relay-config.toml` — bestanden: 24 Tests, 0 fehlgeschlagen; darunter V2-/V3-Import, Wiederherstellung, Deduplizierung und ZIP-Pfadtraversal.
- `cargo +stable test --quiet -p livetab-app bandmap_import_status --locked --offline --config .work/tools/cargo-relay-config.toml` — bestanden: 2/2 neue Status-Tests.
- `cargo +stable test --quiet --workspace --locked --offline --config .work/tools/cargo-relay-config.toml` mit Ninja — bestanden: 27 Tests, 0 fehlgeschlagen, 3 bewusst ignoriert; davon 24 Core- und 3 App-Tests.
- `cargo test -p livetab-app --locked --offline --config .work/tools/cargo-relay-config.toml app_local_registry_selects_fixed_qwen_4b_model -- --ignored --nocapture --test-threads=1` — gezielter ignorierter Assettest separat bestanden: 1/1; der App-Root fällt auf `crates/livetab-app/models` zurück.
- `cargo +stable fmt --all -- --check` — bestanden; die neu hinzugefügten Rust-Funktionen und Tests enthalten deutsche Zweck-/Eingaben-/Ausgabe-Dokumentation.
- `cargo +stable build --quiet -p livetab-app --locked --offline --config .work/tools/cargo-relay-config.toml` auf Windows mit `CMAKE_GENERATOR=Ninja` — bestanden; `target/debug/livetab-app.exe` wurde mit 32.826.368 Bytes erzeugt. Der Standardgenerator Visual Studio scheiterte zunächst wie dokumentiert an MSBuild-Schalter `-j12`; der Ninja-Build war erfolgreich.
- Die Core-Importtests verwenden generierte Testfixtures; in diesem Schritt wurde kein echtes Nutzerarchiv ausgewählt oder importiert. Der native Dialog wurde in den Windows-Build aufgenommen, aber nicht interaktiv bedient.
- Frischer `cargo test --workspace --features cuda --locked --offline --config .work/tools/cargo-relay-config.toml`-Lauf — vor den Rust-Tests an der CMake-CUDA-Compilererkennung gescheitert: `nvcc` kann `vcvars64.bat` nicht initialisieren; die Ausgabe meldet „Die eingegebene Zeile ist zu lang.“ Die vorher dokumentierten CUDA-Smoke-/Benchmarkresultate stammen aus einem früheren erfolgreichen Lauf.

## Frühere Testergebnisse und Messungen

Frühere Windows-Ergebnisse auf RAZOR; Cargo-Abhängigkeiten wurden offline und gelockt verwendet:

- `cargo fmt --all -- --check` — bestanden.
- `cargo test --workspace --features cuda --locked --offline --config .work/tools/cargo-relay-config.toml` — bestanden: 20 Tests, 0 fehlgeschlagen; 3 opt-in-Modell-/Benchmarktests im normalen Lauf übersprungen. Im Ergebnis: 19 Core-Tests und 1 regulärer App-Test.
- Paritätsassertionen in `maps_pitch_classes_to_canonical_names`, `builds_chord_names_and_midi_notes`, `names_keys_and_exposes_chord_intervals`, `validates_time_signatures_and_calculates_positions`, `chord_events_validate_and_calculate_musical_position` und `note_events_derive_pitch_and_validate_midi_values` — im vollständigen Workspace-Lauf bestanden.
- Opt-in `app_local_registry_selects_fixed_qwen_4b_model` — nach Entfernung des 0.8B-Eintrags erneut gezielt ausgeführt und bestanden; das App-Manifest enthält genau ein Modell und der app-lokale 4B-GGUF wird als fester Default erkannt.
- Funktionskommentar-Audit — bestanden: 180 Rust-Funktionsdefinitionen enthalten jeweils Zweck-, Eingaben- und Ausgabehinweis.
- `git diff --check` — bestanden.
- CPU-only-Smoke-Test mit dem festen 4B-GGUF — bestanden: Modellladen 9.709 ms, Inferenz 10.405 ms, 8 Threads, 0 GPU-Layer, 6 erzeugte Tokens und 0,6 Tokens/s für diesen kurzen Prompt.
- CPU-Threadbenchmark mit 32 Tokens — bestanden: 4 Threads 25.095 ms / 1,28 Tokens/s; 6 Threads 20.376 ms / 1,57 Tokens/s; 8 Threads 17.594 ms / 1,82 Tokens/s. Das ist kein Audio-Latenztest.
- CUDA-Smoke- und Laufzeitbenchmark auf NVIDIA GeForce GTX 1650 SUPER (4.095 MiB VRAM) — bestanden. Im identischen 32-Token-Benchmark mit 8 CPU-Threads wurden gemessen: CPU-only 1,82 Tokens/s; 8 GPU-Layer 3,30; 16 GPU-Layer 4,98; 24 GPU-Layer 7,95; 28 GPU-Layer 10,92 Tokens/s. Bei 28 Layern lagen die Messungen für 4/6/8 CPU-Threads bei 9,15/10,06/10,92 Tokens/s.
- VRAM beim GPU-Test: zu Beginn 3.282 MiB frei. 24 Layer belegten 1.910,66 MiB Modellpuffer plus 505 MiB Compute-Puffer (rechnerisch rund 866 MiB Rest); 28 Layer belegten 2.172,35 MiB Modell- plus 505 MiB Compute-Puffer (rund 605 MiB Rest). 28 ist der schnellste getestete Wert, 24 bietet mehr Reserve; keines davon wird ohne automatische VRAM-Prüfung als portabler Standard empfohlen. Der Code-Standard bleibt bei 8 Layern.
- Nach der deutschen Domainänderung wurden der komplette CUDA-Workspace-Test und ein 28-Layer-GGUF-Smoke-Test erneut ausgeführt. Der Smoke-Test bestand: 28/34 Layer ausgelagert, Laden 12,024 s, Inferenz 1,430 s, 6 Tokens, 4,2 Tokens/s für den kurzen Prompt. Frühere Reloads lagen bei 2,663 s; Cache-/Kaltstartbedingungen wurden nicht kontrolliert, deshalb ist die Ladezeit nicht als feste Startzeit zu interpretieren.
- Die Smoke-Tests prüfen Laden und Token-Ausgabe, nicht semantische Antwortqualität; ein kurzer Testlauf begann mit dem Rollentext `user`. Musiktheoretische Antwortqualität und ein produktionsreifer Prompt sind noch offen.
- Nicht ausgeführt: Clippy sowie Builds auf Android/iOS; Audioaufnahme und Echtzeit-Latenz sind noch nicht implementiert oder gemessen.

## Zwei Gesamt-Slices — Umsetzungsstand 2026-11-08

### Slice 1: GUI

- Abgeschlossen: dreispaltige Slint-Workstation mit Projekt-, Audioeingangs-, Analyse-, Timeline-, Tab/Lyrics- und Dev-Observer-Bereichen in `crates/livetab-app/ui/main.slint`.
- Die 27 Slint-Callbacks sind an `AppController`/`app_workflows.rs` angeschlossen; Status- und Ereignisweitergabe erfolgt über die Core-Schnittstellen. Die Slint-Datei wurde mit dem Offline-Parser-Harness syntaktisch validiert.
- Windows-Dateiauswahl und Audioeingangs-/Wiedergabe-Adapter sind im App-Quellcode verdrahtet. Wegen des ausstehenden vollständigen App-Builds wurden native Dialoge und echte Audiogeräte in dieser Runde nicht interaktiv end-to-end verifiziert.

### Slice 2: Logik und Workflows

- Abgeschlossen im Quellcode: getrennte Core-Schicht für WAV-Ein-/Ausgabe, Chroma-/Pitch-DSP, Live-Ringbuffer, Akkord-/Tonartbestimmung samt Alternativen, optimierende Griffbrett-Fingersätze mit ASCII-Tab-Ausgabe, Lyrics-Transkriptions-/Alignment-Worker, synchronisierte Wiedergabe sowie EventBus, JobManager und Observability.
- Die Lyrics-Ausrichtung erhält die erkannte Sprache. Der Modellkatalog behält Qwen3.5-4B Q4_K_M als festen Standard bei; Tests schützen die Manifest-/Registry-Konsistenz.
- Finaler Offline-Corelauf mit der bereits vorhandenen Stable-Toolchain unter `C:\Users\ralle\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin`: `cargo test --manifest-path .work\core-offline-harness\Cargo.toml --all-targets --offline` — **55 bestanden, 0 fehlgeschlagen, 0 ignoriert**, ohne Compilerwarnungen.
- Zusätzlich bestanden: `cargo fmt --all -- --check`, `git diff --check` und der Offline-Slint-Parserlauf. Die beiden Slices wurden vor diesen abschließenden Prüfungen fertiggestellt; es wurde keine globale Toolchain umgestellt.

### Buildgrenze und offene Verifikation

- Der vollständige Workspace-/App-Build ist **nicht bestätigt**. Der Windows-Audiopfad benötigt `wasapi`, das im lokalen `.cargo`-Cache nicht vorhanden ist. Die vorhandene `C:\Users\ralle\.cargo\config.toml` enthält nur eine Linker-Konfiguration und keinen Registry-Relay/Mirror.
- `cargo fetch --locked --manifest-path Cargo.toml` wurde mit der vorhandenen Cargo-Konfiguration versucht, scheiterte jedoch bereits beim Abruf von `https://index.crates.io/config.json` mit SChannel `SEC_E_NO_CREDENTIALS`; daher konnte die fehlende Abhängigkeit nicht geladen und der komplette App-Build nicht erneut verifiziert werden. Es wurde keine TLS-Prüfung abgeschaltet und kein Ersatz-Crate improvisiert.
- Android-/iOS-Builds, native Geräte-/Dialog-Laufzeit, Clippy und reale Live-Audio-Latenz bleiben offen. Die 55 Core-Tests sind ein Offline-Harness-Ergebnis und ersetzen keinen vollständigen Workspace-Build.

## Lokale Vier-Stem-Trennung – Implementierungsstand 2026-11-13

- Basis ist GitHub-`main` `ca891b1e06d4e0df37e3d5a596567b8d28db4cf1`; die lokale Prüfungskopie wurde darauf fast-forwarded. Das konfigurierte `origin` blieb unverändert beim lokalen Bundle `02c482f`. Kein Commit und kein Push.
- Implementiert: rückwärtskompatibles Projektmodell für vier Stem-Tracks; `AppController`-Übernahme; `.dbztab`-Ein-/Ausgabe mit vier WAVs; gepinnter lokaler Python-Worker mit JSON-Lines-Fortschritt, Abbruch und Ausgabevalidierung; expliziter Slint-Downloadfreigabedialog; solo-auswählbare Quellen mit gemeinsamem Playhead; Begrenzung der synchron geladenen WAV- und dekodierten Mehrspurpuffer.
- `cargo check --locked -p livetab-app`: **bestanden** im Linux-Sandboxbuild. Es blieben Compilerwarnungen in Audioaufnahme-/Playbackmodulen, aber keine Fehler.
- `cargo test --locked --workspace`: **bestanden** — App: 12 bestanden, 0 fehlgeschlagen, 3 ignoriert; Core: 58 bestanden, 0 fehlgeschlagen. Die ignorierten Tests erfordern entweder `LIVETAB_TEST_GGUF` oder das lokale Qwen3.5-4B-Modellasset. `cargo fmt --all -- --check` und `git diff --check` bestanden.
- Python-Worker: `python3 -m unittest -v crates/livetab-core/workers/test_stem_worker.py` — **6 bestanden**; `py_compile` für Worker und Tests bestanden.
- Im echten Audiopfad ungeprüft: Python-Fork im verwendeten App-Python installieren, Separatorgewichte beziehen, echte Stem-Trennung ausführen und native Geräte-/GUI-Laufzeit prüfen. In dieser Umsetzung wurden **keine** Modellgewichte heruntergeladen; der Code fragt erst beim Lauf fehlender Modellressourcen nach und lädt nur nach ausdrücklicher Slint-Bestätigung.
- Dieser Linux-Build des Stem-Slices bestätigte nicht Windows-`wasapi`, Android oder iOS. Der Windows-Systemaudio-Code ist inzwischen im folgenden Abschnitt ergänzt; echte Geräte-/GUI-Laufzeit und Latenz sind weiterhin offen.


## Windows-Systemaudio speichern und automatisch analysieren — Stand 2026-11-13

- Der vorhandene Windows-Capture nutzt WASAPI-Loopback am ausgewählten Render-Endpunkt und analysiert Liveframes parallel auf einem separaten DSP-Worker. Das erfasst den Systemmix des Ausgabeendpunkts, keine einzelne YouTube-App oder einen Browser-Tab.
- Die gepinnte `wasapi`-0.26.0-Quellversion wurde geprüft: Render-Endpunkt plus Shared-Capture-Richtung setzt `AUDCLNT_STREAMFLAGS_LOOPBACK`. Das verifiziert die API-Verdrahtung, nicht die tatsächliche Windows-Gerätelaufzeit.
- Neu verkettet: Beim Sichern der bis zu zehn Minuten langen Liveaufnahme wird eine lokale Mono-PCM-WAV geschrieben und danach automatisch die vorhandene vollständige Rust-DSP-Offlineanalyse gestartet. Ergebnisse für Akkorde, Noten, Tonart, Beatgrid und Dauer werden in das aktive Projekt übernommen. Der Job lässt sich abbrechen; die WAV bleibt bei einem Analysefehler nach erfolgreichem Schreiben erhalten.
- Nach terminalem DSP-Auftrag wird für eine tatsächlich gespeicherte WAV ein separater `StemSeparation`-Job gestartet. Er fragt den bestehenden Python-Worker zunächst ohne Downloadfreigabe ab; bei fehlenden Modellressourcen zeigt die UI den vorhandenen expliziten Bestätigungsdialog. Eine Ablehnung erhält WAV und DSP-Ergebnis, lädt nichts herunter und ersetzt keine vorhandenen Stem-Spuren. Ein ausdrücklicher Abbruch der Aufnahme-Nachanalyse unterdrückt den Folgejob.
- Der App-Puffer wird nach dem WAV-Schreiben vor der erneuten WAV-Dekodierung freigegeben. Setup des Stem-Folgejobs läuft außerhalb des UI-Threads. Auf Nicht-Windows-Builds wird keine Windows-Systemaudio-Automatik behauptet.
- Projekt-/Quellwechsel und weitere Aufnahme-/Analyseaktionen sind während dieses Auftrags gesperrt, damit der Folgeworker garantiert die neu gespeicherte Aufnahme und nicht einen inzwischen geänderten Projektpfad erhält.
- `cargo test --locked --workspace`: **bestanden** — App 16 bestanden, 0 fehlgeschlagen, 3 bewusst ignoriert; Core 58 bestanden, 0 fehlgeschlagen. Die beiden zusätzlichen App-Tests prüfen einmaligen Folgejobstart nach terminaler Analyse und Unterdrückung nach Abbruch; bestehende Aufnahmetests sichern WAV-/DSP-Pfad und Abbruch vor dem Schreiben.
- `cargo check --locked -p livetab-app`: **bestanden** im Linux-Sandboxbuild. `cargo fmt --all -- --check` und `git diff --check`: **bestanden**. Vorhandene plattformabhängige Import-/Dead-code-Warnungen in Capture/Playback bleiben.
- `python3 -m unittest -v crates/livetab-core/workers/test_stem_worker.py`: **6 bestanden**; `py_compile` für Worker und Tests bestanden. Diese Tests prüfen das Freigabegate und das Vier-Stem-Mapping isoliert, nicht die tatsächliche Modellausführung in der App.
- **Nicht verifiziert:** echte WASAPI-Geräteaufnahme und Windows-GUI-Laufzeit, YouTube-/Systemton über ein reales Ausgabegerät, Capturelatenz, Audioqualität und tatsächliche Modellgewichte-/Stem-Ausführung im App-Python. Der Linux-Build kompiliert nicht den Windows-`wasapi`-Zweig. Android/iOS bleiben Mikrofonaufnahme statt Systemloopback; Linux-Desktop meldet den fehlenden lokalen Captureadapter.
- Umsetzungsplan: `IMPLEMENTIERUNGSPLAN_2_SYSTEMAUDIO.md`. Kein Commit erstellt und nichts gepusht.

## Separater ASIO-Interfaceeingang — Stand 2026-11-13

- Die Windows-Quellliste zeigt `ASIO ·`-Treiber getrennt von `WASAPI ·`-Renderendpunkten. ASIO öffnet den Interface-Treiber auf einem eigenen Capturethread; WASAPI-Loopback bleibt der Systemmixpfad. Beide speisen den bestehenden Live-DSP- und Mono-WAV-/Analyseworkflow.
- Adapter: `crates/livetab-app/src/asio_capture.rs`; `azo` 0.6.1 (MIT, direkter Treiberzugriff ohne das Steinberg-SDK). Der Treibercallback übergibt Bufferwechsel über einen begrenzten, nicht blockierenden Kanal; Konvertierung und DSP laufen im Worker. Der erste Eingangskanal beziehungsweise das erste Eingangspaar wird unterstützt; übliche lineare PCM-/Floatformate werden konvertiert.
- `cargo check --locked -p livetab-app`: **bestanden** unter Linux; Windows-`cfg` wird dadurch nicht mitkompiliert. `cargo test --locked --workspace`: **bestanden** — App 16 bestanden, 0 fehlgeschlagen, 3 ignoriert; Core 58 bestanden, 0 fehlgeschlagen.
- `cargo check --locked --target x86_64-pc-windows-gnu -p livetab-app` und `cargo build --locked --target x86_64-pc-windows-gnu -p livetab-app`: **bestanden** im Linux-Crossbuild. Der ASIO-/WASAPI-Code und die nativen `llama.cpp`-Bibliotheken wurden mitkompiliert; erzeugt wurde eine Windows-x64-EXE. Dafür waren nur lokale Sandbox-Toolchain-Overrides und ein temporärer MinGW-Kompatibilitätsheader in `.work` nötig; portable Projekt-Builddateien blieben unverändert.
- Ein Windows-Testpaket mit EXE und benötigten DLLs wurde erzeugt; ZIP-Integrität und DLL-Abhängigkeiten wurden geprüft. Der Build ist gestripped, aber kein Release-Build.
- **Nicht verifiziert:** Treiberscan und Echtaufnahme mit echtem installiertem ASIO-Treiber/Interface, Windows-GUI, Audioqualität, Bufferverluste oder reale Latenz. Keine Hardwaremesswerte wurden erhoben. Details und Quellen stehen im `WINDOWS_CROSS_BUILD_STATUS.md` und `IMPLEMENTIERUNGSPLAN_2_SYSTEMAUDIO.md`. Kein Commit erstellt und nichts gepusht.
