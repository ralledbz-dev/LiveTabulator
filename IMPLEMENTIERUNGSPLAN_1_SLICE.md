# Implementierungsplan – Slice 1: Lokale Stem-Trennung mit synchronisierten Spuren

## GitHub-Basis und Abgleich

Vor Beginn stand die lokale Kopie auf `02c482f`; GitHub-`main` war mit `ca891b1` („Complete audio and workflow slices“) um einen Commit voraus. Die Änderungen dieses Slices basieren auf `ca891b1e06d4e0df37e3d5a596567b8d28db4cf1`. Das lokale `origin` bleibt unverändert auf dem Bundle-Commit `02c482f`; es wurde nichts nach GitHub gepusht. Alle Umsetzungsänderungen liegen lokal und sind nicht committet.

Der GitHub-Stand brachte bereits WAV-Import, lokale Audioanalyse, Akkord-/Noten-/Beat-Erkennung, Wiedergabe, Jobverwaltung, `.dbztab`-Export und einen Python-Lyrics-Worker mit. Neu fehlten ein persistentes Stem-Track-Modell, Stem-Dateien im portablen Archiv und die angebundene Separator-Verarbeitung.

## Ziel und Nutzerablauf

Ein Musiker öffnet eine lokale WAV-Datei, startet eine vollständig lokale Trennung, sieht Original und erzeugte Spuren in der Songansicht, kann eine Quelle bei laufendem gemeinsamen Playhead auswählen und das Ergebnis als `.dbztab` speichern und später wieder öffnen.

Für diesen Slice werden **Gesang, Schlagzeug, Bass und übrige Instrumente („Other“)** geliefert. Eigene Gitarren- und Synthesizer-/Klavierspuren werden nicht versprochen: Das gewählte `htdemucs.yaml` ist ein Vier-Stem-Modell.

## Bestätigte Architektur

- **Rust-Core und eine Slint-Oberfläche** bilden die App; es gibt keine zweite PySide6-Oberfläche.
- **Python läuft als lokaler Nebenprozess** und führt den Separator aus. Rust kontrolliert Pfade, Jobstatus, Abbruch, Projektänderung und Ergebnisvalidierung. Der Prozess überträgt JSON-Lines auf stdout und Diagnosen auf stderr.
- **Keine stillen Modell-Downloads und kein Cloud-Fallback.** Fehlen Modellressourcen, zeigt die App die fehlenden Dateien, die vom Fork vorgesehenen Quellen, die nicht vorab zuverlässig bekannte Downloadgröße und den ungeprüften Lizenzstatus. Erst die ausdrückliche Freigabe des Nutzers für diesen Auftrag ermöglicht Netzabrufe. Ablehnung lässt Projekt und Modellbestand unverändert.

## Umgesetzter Umfang dieses Slices

1. **Persistentes Stem-Modell:** `SongProject` speichert vier validierte Stem-Spuren mit Typ, Anzeigename, WAV-Pfad, Dauer, Abtastrate, Kanalzahl und Modellherkunft. Projekte ohne dieses Feld bleiben dank Serde-Default ladbar.
2. **Lokaler Worker:** Der bestätigte Nutzer-Fork `python-audio-separator` ist auf einen konkreten Git-Commit gepinnt. Eingabe sind lokale WAV- und App-Datenpfade; die Workerantworten werden auf Format, erlaubte Stem-Klassen, symlinkfreie Ausgabe, Größe und Zeitabgleich geprüft. Der Prozess lässt sich abbrechen.
3. **Modellzustimmung:** Downloader des Forks werden vor dem Downloadpfad hart blockiert. Bei Bedarf öffnet Slint einen separaten Freigabedialog; nur dessen Zustimmung startet den Worker mit Downloadfreigabe. Quellen, Dateinamen, unbekannte Downloadgröße und ungeprüfter Lizenzstatus werden vor der Entscheidung offengelegt.
4. **Portables Projekt:** `.dbztab` nimmt Stem-WAVs unter kanonischen relativen Namen auf und stellt sie beim Öffnen wieder her. Traversalpfade und zu große Einträge werden abgewiesen; Tests decken den Vier-Stem-Rundtrip ab.
5. **Bedienung und Playback:** Slint zeigt Trennungsstatus, Freigabe, Abbruch und auswählbare Quellen. Original und Stem-WAVs werden beim Start geprüft und im selben nativen Playback-Stream gehalten; ein Quellenwechsel setzt den Playhead nicht zurück. Für parallele Quellen gelten Grenzen von 384 MiB WAV-Dateien und 768 MiB dekodierten Samples. Der aktuelle Slice bietet Quellenwahl/Solo, noch keinen ausgebauten Mehrspur-Mixer mit Lautstärke, Panorama, EQ oder eigenen Wellenform-Editoren.
6. **App-Abhängigkeit:** `serde`/`serde_json` sind direkt in `livetab-app` deklariert und das `Cargo.lock` wurde offline entsprechend aktualisiert. `requirements-stem.txt` pinnt nur den Separator-Fork, keine Modellgewichte.

## Bewusste Grenzen

Nur WAV als Eingabe. MP3, AIFF, OGG und MP4-Audio, Systemaudio-Loopback, Live-Stem-Trennung, zusätzliche Modellklassen, MIDI-/Guitar-Pro-Import sowie Android-/iOS-Paketierung sind nicht Teil dieses Slices. Python und Modellgewichte werden nicht mit der App gebündelt; der Nutzer benötigt ein lokales Python mit der gepinnten Abhängigkeit (bei Bedarf über `LIVETAB_PYTHON` auswählbar). Eine reale Trennung mit heruntergeladenen Gewichten und ein Gerätetest wurden in dieser Runde nicht ausgeführt.

## Quellen und Entscheidungen

- LiveTabulator-Basis: [ralledbz-dev/LiveTabulator](https://github.com/ralledbz-dev/LiveTabulator), Commit `ca891b1e06d4e0df37e3d5a596567b8d28db4cf1`.
- Gewählter Separator-Fork: [ralledbz-dev/python-audio-separator](https://github.com/ralledbz-dev/python-audio-separator), gepinnt auf `4dd3477c0a8f5bfb0fb053c6ed444bb935cb150e` (Paketversion `0.48.0`). Dessen README beschreibt `htdemucs.yaml` als Vier-Stem- und `htdemucs_6s.yaml` als Sechs-Stem-Modell; der Slice bleibt bewusst beim Vier-Stem-Modell.
- Im Fork-Code geprüft: Modellkonfigurationen über `https://github.com/nomadkaraoke/python-audio-separator/releases/download/model-configs`; öffentliche UVR-Modelldateien über `https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models`; Metadaten über `https://raw.githubusercontent.com/TRvlvr/application_data/main/filelists/download_checks.json`; Demucs-Gewichte können laut Modellkonfiguration von `dl.fbaipublicfiles.com/demucs/...` kommen.
- Lizenzstatus der Modellgewichte wird von der App nicht als geprüft behauptet; die UI kennzeichnet ihn ausdrücklich als `unverified by app`.

## Visuelle Folgeleitlinie

Die vom Nutzer übermittelten Referenzen dienen als Leitlinie für spätere UI-Slices: lesbare Takt-/Notendarstellung mit Gitarrentabulatur, Fretboard mit markierten Tönen sowie ein klarer Trackmixer und Player. In diesem Stem-Slice wurde die bestehende Oberfläche nur für Trennung, Freigabe, Status und Quellenwahl ergänzt; ein Redesign nach diesen Referenzen ist nicht Teil der jetzigen Umsetzung.

## Tatsächlicher Teststatus – 2026-11-13

- `cargo check --locked -p livetab-app`: **bestanden** auf Linux; kein Fehler. Der Build meldet Warnungen in den vorhandenen Audioaufnahme-/Playbackmodulen.
- `cargo test --locked --workspace`: **bestanden** — App 12 bestanden, 0 fehlgeschlagen, 3 ignoriert; Core 58 bestanden, 0 fehlgeschlagen. Die drei ignorierten Tests benötigen zwei lokale GGUF-Smoke-Testdateien (`LIVETAB_TEST_GGUF`) bzw. das App-Qwen3.5-4B-Modellasset.
- `python3 -m unittest -v crates/livetab-core/workers/test_stem_worker.py`: **6 bestanden**; `py_compile` für Worker und Testdatei bestanden.
- `cargo fmt --all -- --check` und `git diff --check`: **bestanden**.
- Nicht ausgeführt: echte Separation mit PyTorch/Modellgewichten, Netzdownload nach einem echten UI-Dialog, Laufzeit mit Audiogerät sowie Windows-/Android-/iOS-Builds. Es wurden keine Modellgewichte heruntergeladen. Der vollständige Linux-Workspace wurde kompiliert und getestet; das ersetzt keinen Windows- oder Mobilgeräte-Lauf.
