<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: API_CONTRACTS.md
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

# API Contracts zwischen App und Analysis Worker

## Ziel

Dieses Dokument beschreibt die Schnittstellen zwischen C# Desktop-App und Python Analysis Worker.

## Worker-Protokoll

MVP:

```text
HTTP REST + JSON
```

Später:

```text
gRPC / MessagePack / Shared Memory
```

## Endpunkte

### Health

```http
GET /health
```

Antwort:

```json
{
  "status": "ok",
  "version": "0.1.0",
  "modelsLoaded": true
}
```

### Chord Analysis

```http
POST /analyze/chords
```

Eingabe:

```json
{
  "audioPath": "data/audio/song.wav",
  "mode": "fast"
}
```

Ausgabe:

```json
{
  "jobId": "chords_001",
  "state": "queued"
}
```

### Lyrics Job

```http
POST /analyze/lyrics
```

Eingabe:

```json
{
  "audioPath": "data/audio/song.wav",
  "language": "auto",
  "useStemSeparation": true,
  "wordTimestamps": true
}
```

### Job Status

```http
GET /jobs/{jobId}
```

Antwort:

```json
{
  "jobId": "lyrics_001",
  "state": "running",
  "progress": 0.72,
  "message": "Transcribing vocals"
}
```

### Job Result

```http
GET /jobs/{jobId}/result
```

Antwort enthält je nach Job:

- `chords`
- `beats`
- `key`
- `lyrics`
- `notes`
- `stems`

## Fehlerformat

```json
{
  "error": {
    "code": "MODEL_NOT_FOUND",
    "message": "Das Whisper-Modell wurde nicht gefunden.",
    "details": {
      "expectedPath": "D:/Models/DBZS-LiveTab/asr"
    }
  }
}
```

## Pflichtregel

Jede API-Methode muss dokumentieren:

- Zweck
- Request-Schema
- Response-Schema
- Fehlercodes
- Seiteneffekte
- Logging-Kategorie
