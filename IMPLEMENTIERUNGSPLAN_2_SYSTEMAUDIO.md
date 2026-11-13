# Implementierungsplan – Slice 2: Systemaudio aufnehmen und anschließend analysieren

## Ziel

Windows-Systemton, beispielsweise die Wiedergabe eines YouTube-Videos am gewählten Ausgabeendpunkt, soll lokal aufgenommen werden. Nach dem Speichern der Aufnahme soll die App nicht beim bloßen WAV-Export stehenbleiben, sondern die vorhandene vollständige Rust-DSP-Offlinanalyse ausführen und das Ergebnis in Projekt und Oberfläche übernehmen. Anschließend startet auch der vorhandene lokale Python-Worker die Vier-Stem-Trennung.

Die App zeichnet den ausgewählten WASAPI-Render-Endpunkt auf. Das ist der Systemmix dieses Endpunkts, keine gezielte Aufnahme nur eines Browser-Tabs oder einer einzelnen Anwendung.

## Nutzerablauf

1. Unter Windows den Geräte-Scan ausführen und den gewünschten Render-Endpunkt auswählen oder den Standardendpunkt verwenden.
2. „Live starten“ öffnet den lokalen WASAPI-Loopback-Capture. Separater DSP-Code aktualisiert Livepegel, Waveform und laufende Akkorderkennung.
3. „WAV aufnehmen“ puffert bis zu zehn Minuten Mono-Audio; die Speicheraktion wählt den lokalen Zielpfad.
4. Ein abbrechbarer Hintergrundjob schreibt die WAV lokal, zeigt nach erfolgreichem Schreiben Pfad, Dauer und Waveform an und startet anschließend die vorhandene vollständige Offlineanalyse. Diese übernimmt Akkorde, Noten, Tonart, Beatgrid und Dauer in das aktive Songprojekt.
5. Nach Ende des DSP-Auftrags (auch wenn DSP fehlschlägt, sofern die WAV gespeichert wurde) startet Windows den bestehenden Python-Worker mit gesperrten Downloads. Sind Modellressourcen vorhanden, werden vier geprüfte WAV-Stems übernommen; fehlen sie, fragt die App explizit nach, bevor der Worker einen Download ausführen darf.
6. Bei Ablehnung werden keine Modelldateien geladen und keine bisherigen Stem-Spuren ersetzt; gespeicherte WAV und DSP-Ergebnis bleiben bestehen. Ein Abbruch des Aufnahme-/DSP-Auftrags unterdrückt den Stem-Folgejob.

## Umsetzung und Entscheidungen

- `crates/livetab-app/src/audio_capture.rs` stellt bereits den Windows-Loopback-Capture über WASAPI bereit; die Callback-/Capture-Schicht speist bounded Pakete in den separaten Live-DSP-Worker. Die vorhandene Zehn-Minuten-Grenze bleibt unverändert.
- `crates/livetab-app/src/app_workflows.rs` verkettet den Abschluss der Pufferaufnahme mit WAV-Schreiben und `AppController::analyze_audio_file_with_cancel`; nach terminalem DSP-Status startet für eine gespeicherte WAV separat der bestehende Vier-Stem-Workflow. Analysefortschritt und Abbruch laufen über den vorhandenen JobManager. Der Aufnahmesamplepuffer wird nach dem WAV-Schreiben freigegeben, bevor die Datei für die Analyse dekodiert wird.
- Während Aufnahme-Nachanalyse bzw. Stem-Setup sind konkurrierende Projekt-/Quellwechsel gesperrt; Live-Capture und der separate DSP-Worker bleiben unabhängig vom LLM-Worker.
- Der Stem-Worker wird zuerst mit `allowModelDownload=false` ausgeführt. Der bestehende JSON-Lines-Worker meldet fehlende Ressourcen und öffnet den vorhandenen Slint-Freigabedialog. Erst die konkrete Bestätigung startet denselben Auftrag mit Downloadfreigabe; Ablehnung löscht den ausstehenden Auftrag und verändert weder Modellbestand noch bisherige Stem-Spuren.
- `crates/livetab-app/ui/main.slint` kennzeichnet die Windows-Aufnahmeaktion als „WAV sichern · DSP + Stems“; die Statusanzeige meldet Sicherung, Analyse, Stem-Start und Freigabe getrennt.
- Der Offlinepass verwendet bestehende lokale Core-Funktionen. Es wurden weder ein Cloud-Fallback noch neue Analysemodelle, Modellgewichte oder Netzwerkabrufe ergänzt.
- Es gibt keine app-spezifische YouTube-Integration. Die Quelle ist der vom Nutzer gewählte Windows-Ausgabeendpunkt.

## Plattformgrenzen

- **Windows:** WASAPI-Render-Loopback erfasst den Systemmix des ausgewählten Endpunkts; für gespeicherte Liveaufnahmen wird die Stem-Trennung automatisch angehängt. Das echte Gerät und die GUI wurden in dieser Linux-Sandbox nicht interaktiv geprüft.
- **Android/iOS:** Der vorhandene Captureadapter öffnet das lokale Mikrofon. Das ist keine Aufnahme des internen Systemtons; die automatische Liveaufnahme-Stem-Verarbeitung ist in diesem Slice Windows-only.
- **Linux-Desktop:** Der bestehende Capturepfad meldet klar, dass keine lokale Desktop-Audioquelle integriert ist; die automatische Stem-Folgeaktion ist Windows-only.

## Verifikation

- `cargo test --locked --workspace`: **bestanden** — App 16 bestanden, 0 fehlgeschlagen, 3 bewusst ignoriert; Core 58 bestanden, 0 fehlgeschlagen. Zwei zusätzliche App-Tests sichern den einmaligen Folgejobstart nach terminaler Analyse und dessen Unterdrückung bei Abbruch.
- `cargo check --locked -p livetab-app`: **bestanden** für den Linux-Build. Es bleiben vorhandene plattformbedingte Import-/Dead-code-Warnungen in Audioaufnahme- und Playbackmodulen.
- `python3 -m unittest -v crates/livetab-core/workers/test_stem_worker.py`: **6 bestanden**; `py_compile` für Worker und Tests bestanden. Verifiziert die Worker-Freigabe- und Mappingregeln isoliert, nicht echte Modellinferenz.
- `cargo fmt --all -- --check` und `git diff --check`: **bestanden**.
- Nicht verifiziert: echte WASAPI-Geräteaufnahme, tatsächliche YouTube-/Systemtonaufnahme, Windows-GUI-Laufzeit, Capturelatenz und Audioqualität sowie die echte Python-Fork-/Modellinstallation und Stem-Ausführung. Ein erfolgreicher Linux-Build bestätigt nicht den Windows-`wasapi`-Pfad.

## Aktueller Git-Stand

Die Änderungen liegen lokal im bestehenden Arbeitszweig. Es wurde kein Commit erstellt und nichts gepusht.


## ASIO – separater Windows-Interfaceeingang (umgesetzt)

ASIO ist als eigene Windows-Quelle neben WASAPI integriert. Der Quellwähler markiert Einträge mit `ASIO ·` beziehungsweise `WASAPI ·`: WASAPI bleibt der vorhandene Loopback-Pfad für den Systemmix eines Render-Endpunkts; ASIO erfasst Eingänge eines registrierten ASIO-Audiointerfaces und ist keine allgemeine „YouTube-Systemaudio“-Quelle. Die voreingestellte Auswahl bleibt WASAPI-Standardausgabe.

`crates/livetab-app/src/asio_capture.rs` verwendet `azo` 0.6.1 direkt. `azo` veröffentlicht eine MIT-Lizenz und greift laut Projektbeschreibung ohne Steinberg-ASIO-SDK auf die COM-Objekte registrierter Treiber zu. Die Echtzeitcallbacks stellen Bufferwechsel über einen begrenzten, nicht blockierenden Kanal an den Captureworker zu; Samplekonvertierung, Mono-Mischung, bestehende DSP-Pipeline und WAV-Aufnahme laufen außerhalb des Treibercallbacks. Erfasst werden der erste Eingangskanal beziehungsweise das erste Eingangspaar; unterstützte lineare PCM-Formate werden in Float umgewandelt. Der Treiber wird auf einem eigenen Capturethread geöffnet, initialisiert, gestartet, gestoppt und freigegeben.

Diese Umsetzung bindet weder das Steinberg-ASIO-SDK noch `asio-sys` ein. Damit wird für den neuen ASIO-Pfad keine GPLv3-SDK-Integration in das MIT-Projekt vorgenommen; Steinbergs separate proprietäre SDK-Lizenz wird ebenfalls nicht benutzt. Die Aufnahme eines echten Treibers/Geräts und die gerätespezifischen Treiberbedingungen bleiben dennoch gesondert zu prüfen.

**Verifikation:** `asio_capture.rs` und `audio_capture.rs` bestanden gemeinsam einen isolierten `x86_64-pc-windows-gnu`-Zielcompile mit `azo`, `crossbeam-channel`, `cpal`, `wasapi` und dem Rust-Core; `--tests` typprüfte außerdem den ASIO-Testcode, führte ihn aber nicht aus. Der vollständige App-Windows-Compile konnte wegen des vorhandenen `llama-cpp-sys-4`-Buildscripts nicht bis zur App gelangen: Clang findet `stdbool.h` in der Sandbox-MinGW-Zielkonfiguration nicht. Linux-Appbuild und Workspace-Tests bestanden; Linux allein kompiliert den Windows-Zweig nicht. **Nicht geprüft:** ASIO-Laufzeit mit echtem Treiber/Interface, Windows-GUI, tatsächliche Latenz oder Audioqualität.

## Technische Referenzen

- [Microsoft: WASAPI Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) – beschreibt Capture des auf einem Render-Endpunkt wiedergegebenen Systemmixes im Shared Mode.
- [wasapi 0.26.0: `AudioClient`](https://docs.rs/wasapi/0.26.0/wasapi/struct.AudioClient.html) – dokumentiert Render-Endpunkt plus Capture-Richtung; die gepinnte Crate-Quelle setzt für `(Render, Capture, Shared)` `AUDCLNT_STREAMFLAGS_LOOPBACK`.
- [Steinberg: ASIO Open-Source-Lizenz](https://www.steinberg.net/developers/asiosdk-open/) und [proprietäre SDK-Lizenzinformationen](https://www.steinberg.net/developers/prorietary-sdk/) – Grundlage für die offene Lizenz-/Distributionsentscheidung.
- [`azo` 0.6.1 auf docs.rs](https://docs.rs/azo/0.6.1/azo/) und [LastExceed/azo](https://github.com/LastExceed/azo) – MIT-Metadaten, Treiber-API sowie ausdrückliche Abgrenzung zur Steinberg-SDK-Wrappercrate.
