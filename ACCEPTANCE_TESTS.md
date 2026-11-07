<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: ACCEPTANCE_TESTS.md
 Version: 0.1.0
 Stand: 2026-05-23
 Status: Konzept / Entwicklungsgrundlage
 Autor: Division By Zeros (DBZS) / Ralle
 Sprache: Deutsch
===============================================================================

Zweck:
Dieses Dokument beschreibt einen Teilbereich des Projekts „DBZS LiveTab Analyzer“.
Alle technischen Beschreibungen, Funktionen, Methoden, Prozeduren und Workflows
sind so zu dokumentieren, dass sie für Entwickler, KI-Agenten und spätere
Wartung nachvollziehbar bleiben.

Dokumentationspflicht:
- Jede Sourcecode-Datei erhält einen DBZS-Header.
- Jede öffentliche Klasse, Funktion, Methode, Prozedur und Pipeline wird auf Deutsch dokumentiert.
- Jede Funktion beschreibt Zweck, Eingaben, Ausgaben, Fehlerfälle und wichtige Abhängigkeiten.
- UI-Code enthält keine Businesslogik.
- Fachlogik liegt in Core-, Service-, Engine- oder Agent-Modulen.
===============================================================================
-->

# Acceptance Tests

## Ziel

Dieses Dokument definiert prüfbare Ergebnisse für die Entwicklung.

## Audio Capture

- Die App listet Windows-Audioausgabegeräte.
- Die App startet WASAPI Loopback Capture.
- Die Pegelanzeige reagiert auf laufende Musik.
- Eine WAV-Aufnahme kann gespeichert und wiedergegeben werden.
- Capture-Fehler erscheinen im Dev Observer.

## Chord Detection

- Eine einfache WAV mit `Am - F - C - G` erzeugt eine passende Chord Timeline.
- Jeder erkannte Akkord besitzt Confidence.
- Unsichere Akkorde werden anders markiert.
- Nutzerkorrektur überschreibt automatische Erkennung.

## Key Detection

- Aus einer stabilen Akkordfolge wird eine wahrscheinliche Tonart erzeugt.
- Alternative Tonarten werden angezeigt.
- Tonartwechsel werden zeitlich gespeichert.

## Beatgrid

- BPM wird erkannt.
- Takte werden angezeigt.
- Downbeat kann korrigiert werden.
- Korrektur verschiebt das Beatgrid nachvollziehbar.

## Lyrics

- Lyrics Job startet als separater Job.
- Fortschritt wird angezeigt.
- Ergebnis enthält Lines und Words.
- Words besitzen Start/End/Confidence.
- Words werden auf Takt/Beat gemappt.
- Nutzer kann Text korrigieren.

## Tab Engine

- Standardstimmung EADGBE wird unterstützt.
- NoteEvents werden in TabNotes umgewandelt.
- Easy Mode erzeugt spielbare Lagen.
- Chord Mode erzeugt Akkordbegleitung.
- TabRenderer zeigt Takte korrekt.

## Playback

- Play/Pause/Stop funktionieren.
- Cursor läuft synchron.
- Metronom läuft im gewählten Tempo.
- Loopbereich wiederholt korrekt.
- Lyrics-Wort wird zur richtigen Zeit markiert.

## Export

- Projekt kann als `.dbztab` gespeichert werden.
- TXT Tab Export enthält Akkorde, Lyrics und Tabs.
- LRC Export enthält Zeitstempel.
- MIDI Export enthält Notenereignisse.
