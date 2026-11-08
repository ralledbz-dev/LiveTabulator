# Rust/Slint-Neuaufbau: Runtime- und Plattformrecherche

Stand: 2026-11-07

## Festgelegte Architekturgrenze

- Desktop-App und Core werden mit Rust/Slint umgesetzt.
- llama.cpp ist die lokale Runtime für GGUF-Sprachmodelle (Erklärungen, Musiktheorie, Übehinweise), nicht für zeitkritische Chord-/Beat-/Audio-DSP.
- ExecuTorch ist ein mögliches On-Device-Backend für ausgewählte PyTorch-Modelle, die sich nachweislich exportieren und auf den Zielgeräten korrekt ausführen lassen. Das Exportartefakt ist `.pte`; Operatorabdeckung und Genauigkeit müssen pro Modell validiert werden.
- Ein lokaler Python-Worker bleibt als begrenzte Ausweichintegration für Spezialmodelle zulässig, wenn kein geeigneter Rust-/ExecuTorch-Pfad besteht.
- Mobile Systemaudio-Erfassung wird nicht als plattformübergreifend garantiert. Je Plattform werden App-Audio, Mikrofon, Import und – nur wo erlaubt – Playback-Capture getrennt behandelt.
- Der Code kann Runtime und Manifest enthalten. Große Modellgewichte benötigen eine eigene Lizenz-/Größenprüfung und sollen nicht ungeprüft in Git übernommen werden.

## Verifizierte externe Fakten

### Slint und Zielplattformen

- Slint unterstützt Rust-Desktop-Apps; externe `.slint`-Dateien werden über `slint-build`/`build.rs` kompiliert.
  - https://docs.slint.dev/latest/docs/slint/guide/getting-started/
  - https://docs.slint.dev/latest/docs/rust/slint/
- Slint dokumentiert Android-Rust-Apps als `cdylib`, mit Android-Activity-Backend und Android SDK/NDK/CMake.
  - https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/android/
- Slint dokumentiert iOS-Rust-Apps; zum Entwickeln und Bauen werden macOS und Xcode benötigt. Daher kann ein Windows-PC allein einen nativen iOS-Build nicht abschließend verifizieren.
  - https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/ios/
- Slints kostenlose Royalty-free-Lizenz deckt Desktop-/Mobile-/Web-Apps ab, verlangt aber einen sichtbaren `AboutSlint`-Widget in einem vom Hauptmenü erreichbaren Über-Dialog/-Bildschirm oder einen Slint-Badge. Die Projektdateien können unter MIT bleiben.
  - https://github.com/slint-ui/slint/blob/master/LICENSE.md
  - https://github.com/slint-ui/slint/blob/master/FAQ.md
  - https://docs.slint.dev/latest/docs/slint/reference/std-widgets/misc/aboutslint/

### Audioaufnahme auf Mobilgeräten

- Android `AudioPlaybackCapture` benötigt `RECORD_AUDIO` und eine ausdrückliche `MediaProjection`-Nutzerfreigabe. Ob eine andere App erfasst werden darf, hängt unter anderem von deren Audio-Nutzung und Capture-Policy ab; der Nutzer kann die Freigabe widerrufen.
  - https://developer.android.com/media/platform/av-capture
- Apple beschreibt ReplayKit für Bildschirmaufnahme/-streaming sowie Audio der eigenen App und Mikrofon. Das ist keine allgemeine Zusage für den Zugriff einer App auf Audioausgabe beliebiger anderer iOS-Apps.
  - https://developer.apple.com/documentation/replaykit

### llama.cpp und GGUF

- llama.cpp ist eine C/C++-Runtime mit CPU/GPU-Backends, Quantisierung, Apple-Metal-Unterstützung und Android-Buildpfad über CMake/Android NDK.
  - https://github.com/ggml-org/llama.cpp/blob/master/README.md
  - https://github.com/ggml-org/llama.cpp/blob/master/docs/android.md
- `llama-cpp-2` wurde als Kandidat geprüft, aber wegen des dokumentierten Hinweises auf nicht vollständig sichere Wrapper nicht ausgewählt.
  - https://crates.io/crates/llama-cpp-2
- Als initialer Runtime-Adapter ist `llama-cpp-4` 0.11.0 gewählt: Safe-Rust-Wrapper über llama.cpp ohne optionale RPC-/Multimodal-Features; der Windows-Prototyp verwendet den dokumentierten Shared-Linkmodus mit CPU/OpenMP und lokaler Host-CPU-Optimierung. Das Crate selbst nennt weiterhin einzelne zu beachtende Safety-Vorbedingungen, daher bleibt die Verwendung in einem eigenen Adaptermodul gekapselt.
  - https://crates.io/crates/llama-cpp-4
  - https://docs.rs/llama-cpp-4/latest/llama_cpp_4/
  - https://crates.io/crates/llama-cpp-sys-4
- `llama-cpp-sys-4` dokumentiert die Option `dynamic-link` und eincheckbare CMake-Optionen unter Workspace-Metadaten. Der Windows-Prototyp schaltet damit ungenutzte llama.cpp-HTTPS-Unterstützung über `LLAMA_OPENSSL=OFF` ab; Android-/iOS-Build und Runtime-Paketierung bleiben separat zu verifizieren.
  - https://github.com/eugenehp/llama-cpp-rs/blob/main/llama-cpp-sys-4/README.md
- Der einmalige Vollscan auf RAZOR fand ein bereits lokales Qwen3.5-4B-GGUF: `D:\Models\ModelVault\models\agentic-coding\qwen3.gguf` (2,783,447,520 Bytes; SHA-256 `C1268EF0EC3B0316EA62C081645D7BBAF80CF76288E4F6A590C22A073D64C920`). Die echte GGUF-Metadatenlesung ergab `general.name=Qwen3.5 4b Agentic Coder v4`, `general.architecture=qwen35`, `general.finetune=agentic-coder` und `general.license=apache-2.0`.
- Das GGUF nennt als Quantisierungs-/Modellseite https://huggingface.co/mradermacher/qwen3.5-4b-agentic-coder-v4-GGUF; deren Modellkarte nennt Apache-2.0 und listet Q4_K_M mit 2.78 GB als schnelle, empfohlene Quantisierung. Das Quell-Finetune unter https://huggingface.co/mahiatlinux/qwen3.5-4b-agentic-coder-v4 ist laut Modellkarte Apache-2.0 und basiert auf `unsloth/Qwen3.5-4B`.
- Die Datei ist lokal als `qwen3.gguf` benannt; im selben Modellordner lag kein mmproj-Sidecar. Bis ein nutzbares mmproj geprüft und gepaart ist, wird sie nur als Textmodell, nicht als multimodales Modell geführt.
- Qwen3.5-4B Agentic Coder v4 Q4_K_M ist bytegenau nach `crates/livetab-app/models/llm/Qwen3.5-4B-Q4_K_M.gguf` kopiert und als einziges Assistant-Standardmodell festgelegt. Größe: 2,783,447,520 Bytes; SHA-256 `C1268EF0EC3B0316EA62C081645D7BBAF80CF76288E4F6A590C22A073D64C920`. Das zuvor app-lokale Qwen3.5-0.8B Q4_0 wurde nach bestandener 4B-CPU-/CUDA-Verifikation entfernt; die externe Modellablage wurde nicht verändert. App-Modellgewichte bleiben von Git ausgeschlossen.
- Die Modell-Registry durchsucht das App-Modellverzeichnis und auf Windows zusätzlich `D:\Models` rekursiv (ohne Symlinks zu folgen). GGUF-Header und Architektur werden begrenzt und fail-closed geprüft; nicht-textuelle GGUF-Artefakte werden nicht als Chatmodelle angeboten. Beim App-Start wird nur der App-Modellordner geprüft; externe Wurzeln werden erst auf ausdrückliche Nutzeraktion durchsucht.
- Eine separate Arm-Referenzquantisierung nennt 504.8 MB und 1109 MB Spitzen-RAM auf einem vivo X300, ist aber ausdrücklich als Referenz, nicht als produktionsfertige/unterstützte Lösung beschrieben. Diese Zahlen sind kein Messwert für RAZOR oder ein Nutzergerät.
  - https://developer.arm.com/ai/models/hugging-face/Arm/qwen3-5-0-8b-q4-k-m-llamacpp-vivo-x300/qwen3.5-0.8b-q4_k_m-gguf

### ExecuTorch

- ExecuTorch exportiert PyTorch-Modelle via Python-API zu `.pte` und verlangt einen erfolgreichen Export bzw. eine Prüfung des Modells gegen die Ziel-Operatoren.
- Die aktuelle Dokumentation nennt XNNPACK als CPU-Backend (Android/iOS und weitere Ziele) und Core ML als iOS-Backend, das je nach Modell CPU, GPU und Apple Neural Engine nutzen kann.
  - https://docs.pytorch.org/executorch/stable/getting-started.html
  - https://docs.pytorch.org/executorch/stable/android-xnnpack.html
  - https://docs.pytorch.org/executorch/stable/ios-coreml.html

## Lokale Buildvoraussetzungen, direkt auf RAZOR ermittelt

- Rust stable 1.98.1 ist im Benutzerprofil bereits installiert; für diese Prüfung wurde keine Toolchain neu installiert.
- Visual Studio 18 Community mit MSVC und Windows SDK ist vorhanden.
- CMake 4.4.2 und Clang 22.1.8 sind vorhanden.
- WMI-Hardwareabfragen schlagen in dieser Agentenausführung mit „Zugriff verweigert“ fehl; `nvidia-smi` liefert jedoch GPU-Werte: NVIDIA GeForce GTX 1650 SUPER, 4,096 MiB VRAM, davon bei Prüfung 3,930 MiB frei; installierter CUDA-Compiler 13.3.33; 12 für den Prozess verfügbare logische CPUs. Der gesamte System-RAM ist weiterhin nicht verifiziert.
- Die offizielle llama.cpp-Bauanleitung aktiviert CUDA über `-DGGML_CUDA=ON`; sie stellt klar, dass der CPU-Backend standardmäßig ebenfalls mitgebaut wird, und dokumentiert eine explizite `CMAKE_CUDA_ARCHITECTURES`-Wahl. Wegen 4 GiB VRAM wird für Qwen3.5-4B nur partieller Layer-Offload separat gemessen; Voll-Offload wird nicht vorausgesetzt.
  - https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md

## Lizenzspur für das Projekt

Die vorhandene MIT-Lizenz wird nicht geändert. Für Slint wird für die Desktop-Auslieferung die Royalty-free-Option mit erreichbarem `AboutSlint`-Hinweis eingeplant. Jede Modellkarte behält eine separate Herkunfts- und Lizenzangabe; Runtime-Lizenz und Modelllizenz sind getrennte Prüfungen.

## Lokale BANDMAP-Referenz und Portentscheidung — 2026-11-08

- Als Formatreferenz wurde ausschließlich das lokal vorhandene `D:\Dev\LiveTabulator\Tabulator-Live-main.zip` geprüft. Das direkte Original-Remote `https://github.com/devdbzemusic/Tabulator-Live.git` war im vorherigen Abruf nicht verfügbar; das Archiv wird deshalb nicht als notwendigerweise aktueller GitHub-Stand ausgegeben.
- Gelesen wurden die BANDMAP-Format-/V3-Beschreibungen und die portable V3-Archivimplementation im Archiv. Sie dienten nur dazu, Containerverhalten und Importreihenfolge abzugleichen; C#- und Kotlin-Quelltexte wurden weder übernommen noch kopiert.
- Portiert wurde als neue Rust-Implementierung: V1/V2-Recovery über Java-Properties und V3-Portable über JSON-Manifest plus SQLite-Datei. Medien-/Analyseeinträge bleiben unverändert unter `data/` erhalten; die App zeigt beim Start das zuletzt importierte Projekt.
- Sicherheitsentscheidung: Namen, Windows-Sonderfälle, Symlinks/Sonderdateien, Archiv-/Eintrags-/Entpackgrößen und der SQLite-Kopf werden vor dem Entpacken validiert. Ein projektbezogener SHA-256 verhindert doppelte lokale Projekte; Staging plus atomarer Umzug verhindert halbfertige Zielordner.
- Bediengrenze dieses Schritts: Die Slint-Oberfläche verwendet ein `.bandmap`-Pfadfeld und einen Hintergrundimport; Dateiauswahldialog, Bearbeitung des Musik-Domänenmodells und weitere Workflows bleiben spätere Schritte.

## G:\Dev\LIVETAB: read-only Domänenreferenz

- Ausschließlich gelesen wurden die Legacy-Typen `Chord`, `ChordEvent`, `ChordQuality`, `KeySignature`, `LyricsWord`, `NoteEvent`, `NoteName`, `Song`, `TabNote` und `TimeSignature` unter `G:\Dev\LIVETAB\DBZS.LiveTab.Core`; die C#-Quelle wurde nicht verändert und kein C#-Quelltext in Rust kopiert.
- Die geprüften Legacy-Projekte enthielten Domänenmodelle, aber keine implementierte Audio-, Analyse- oder Playback-Engine. Der Rust-Core wird daher neu entworfen und nicht als mechanischer Quellport behandelt.
- Neue Rust-Typen liegen in `crates/livetab-core/src/domain.rs` und `crates/livetab-core/src/song.rs`; `lib.rs` exportiert sie für App- und Analyse-Consumers. Dazu gehören zusätzlich `DomainError` und `MusicalPosition` als gemeinsame Validierungs-/Zeitbasis. Die Legacy-B/H-Semantik ist explizit abgebildet: `NoteName::H` aliasiert die internationale Tonklasse B, `as_german_str()` zeigt Halbton 10 als B und Halbton 11 als H, und Akkorde bieten `german_name()`.
- Die neuen Typen validieren Zeitbereiche, BPM, MIDI-Werte, Konfidenzen, Saitennummern und Taktarten vor Verwendung. Akkord-, Noten-, Lyrics- und Tabereignisse verwenden dieselbe Sekunden-/Taktpositionslogik; bestehende JSON-Feldnamen werden mit Serde-Umbenennungen abgebildet.
- Rust-Core-Abdeckung: 19 Unit-Tests bestanden, darunter Song-Defaults/Dauer, Akkordereignisse, MIDI-Tonhöhen, Lyrics-Zeitpositionen, Tabulaturdaten und Fehlergrenzen.
