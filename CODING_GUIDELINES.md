<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: CODING_GUIDELINES.md
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

# DBZS Coding Guidelines

## Ziel

Alle Sourcecode-Dateien müssen konsistent, wartbar und für KI-Agenten verständlich dokumentiert sein.

## DBZS Source Header

Jede Sourcecode-Datei beginnt mit einem Header:

```csharp
// ============================================================================
// Division By Zeros (DBZS)
// Projekt: DBZS LiveTab Analyzer
// Datei: <DATEINAME>
// Zweck: <KURZE BESCHREIBUNG>
// Sprache: C#
// Erstellt: <DATUM>
// Hinweise:
// - UI enthält keine Businesslogik.
// - Fachlogik liegt in Services, Engines, Agents oder Core-Modulen.
// - Alle öffentlichen Funktionen sind auf Deutsch dokumentiert.
// ============================================================================
```

Für Python:

```python
# ============================================================================
# Division By Zeros (DBZS)
# Projekt: DBZS LiveTab Analyzer
# Datei: <DATEINAME>
# Zweck: <KURZE BESCHREIBUNG>
# Sprache: Python
# Erstellt: <DATUM>
# Hinweise:
# - Keine stillen Fehler.
# - Jeder Analysejob meldet Fortschritt und Fehler.
# - Alle Funktionen sind auf Deutsch dokumentiert.
# ============================================================================
```

## Dokumentationspflicht für Funktionen

Jede Funktion/Methode/Prozedur dokumentiert:

- Zweck
- Parameter
- Rückgabewert
- Fehlerfälle
- Seiteneffekte
- Abhängigkeiten
- wichtige interne Logik

## C# Beispiel

```csharp
/// <summary>
/// Startet die WASAPI-Loopback-Aufnahme für das ausgewählte Windows-Ausgabegerät.
/// </summary>
/// <param name="deviceId">
/// ID des Audioausgabegeräts, das überwacht werden soll.
/// </param>
/// <param name="bufferMilliseconds">
/// Größe des internen Aufnahmebuffers in Millisekunden.
/// </param>
/// <returns>
/// Gibt eine laufende Capture-Session zurück, die Frames an den AudioRingBuffer liefert.
/// </returns>
/// <exception cref="AudioDeviceNotFoundException">
/// Wird ausgelöst, wenn das angeforderte Gerät nicht gefunden wurde.
/// </exception>
/// <remarks>
/// Diese Methode darf die UI nicht blockieren. Alle Statusänderungen werden über
/// den EventBus und den Dev Observer gemeldet.
/// </remarks>
public Task<AudioCaptureSession> StartLoopbackCaptureAsync(
    string deviceId,
    int bufferMilliseconds,
    CancellationToken cancellationToken)
{
    ...
}
```

## Python Beispiel

```python
def transcribe_lyrics(audio_path: Path, language: str | None) -> LyricsResult:
    """
    Transkribiert Liedtext aus einer Audiodatei und erzeugt Wort-Zeitstempel.

    Args:
        audio_path: Pfad zur zu analysierenden Audiodatei oder zum Vocals-Stem.
        language: Optionale Sprachvorgabe. None bedeutet automatische Erkennung.

    Returns:
        LyricsResult mit Zeilen, Wörtern, Zeitstempeln und Confidence-Werten.

    Raises:
        ModelNotFoundError: Wenn das konfigurierte ASR-Modell fehlt.
        AudioDecodeError: Wenn die Audiodatei nicht gelesen werden kann.

    Hinweise:
        Diese Funktion wird nur in einem Analysejob ausgeführt und darf den
        Live-Audio-Thread niemals blockieren.
    """
```

## Verboten

- UI-Businesslogik
- stille Exceptions
- Magic Numbers ohne Kommentar
- unklare Modellpfade
- nicht dokumentierte Datenumwandlungen
- nicht geloggte Analysejobs
- Platzhalter/Stubs in produktiven Pfaden

## Pflicht

- Tests für Core-Logik
- Logs für Pipelines
- deutsche Kommentare bei nicht-trivialer Logik
- eindeutige Dateinamen
- klare Modulgrenzen
