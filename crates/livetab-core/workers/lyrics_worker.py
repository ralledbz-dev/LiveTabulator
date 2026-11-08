# =============================================================================
# Division By Zeros (DBZS)
# Projekt: DBZS LiveTabulator
# Datei: lyrics_worker.py
# Zweck: Optionale CPU-basierte Offline-Transkription mit faster-whisper und
#        Wort-Zeitstempeln über ein JSON-stdin/stdout-Protokoll.
# Sprache: Python
# Hinweise:
# - Ausschließlich absolute lokale Audiodatei- und Modellordnerpfade akzeptieren.
# - Kein Netzwerk-Fallback, keine Modell-ID-Downloads und keine stillen Fehler.
# =============================================================================
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any


def antwort_schreiben(antwort: dict[str, Any]) -> None:
    """
    Schreibt eine einzelne Workerantwort als JSON auf stdout.

    Args:
        antwort: JSON-kompatibles Ergebnisobjekt mit Erfolg, Wörtern oder Fehler.

    Returns:
        None; stdout enthält genau eine JSON-Zeile.

    Raises:
        OSError: Wenn die Ausgabe nicht geschrieben werden kann.
    """
    sys.stdout.write(json.dumps(antwort, ensure_ascii=False, separators=(",", ":")))
    sys.stdout.write("\n")
    sys.stdout.flush()


def anfrage_lesen() -> dict[str, Any]:
    """
    Liest und validiert genau eine JSON-Anfrage von stdin.

    Args:
        Keine; die Anfrage wird vom Rust-Lyrics-Adapter bereitgestellt.

    Returns:
        Dekodiertes Objekt mit audioPath, modelPath, language und wordTimestamps.

    Raises:
        ValueError: Wenn die JSON-Struktur oder die Pflichtfelder ungültig sind.
    """
    rohtext = sys.stdin.read()
    if not rohtext.strip():
        raise ValueError("Der Lyrics-Worker erhielt eine leere Anfrage.")
    anfrage = json.loads(rohtext)
    if not isinstance(anfrage, dict):
        raise ValueError("Die Lyrics-Anfrage muss ein JSON-Objekt sein.")
    for feld in ("audioPath", "modelPath"):
        if not isinstance(anfrage.get(feld), str) or not anfrage[feld]:
            raise ValueError(f"Pflichtfeld {feld} fehlt.")
    return anfrage


def lokale_pfade_pruefen(anfrage: dict[str, Any]) -> tuple[Path, Path]:
    """
    Prüft Audio und Modell als bereits vorhandene absolute lokale Pfade.

    Args:
        anfrage: Validiertes Requestobjekt mit audioPath und modelPath.

    Returns:
        Tupel aus Audiodateipfad und Modellverzeichnis.

    Raises:
        ValueError: Wenn ein Pfad relativ, nicht vorhanden oder vom falschen Typ ist.
    """
    audio = Path(anfrage["audioPath"]).expanduser()
    modell = Path(anfrage["modelPath"]).expanduser()
    if not audio.is_absolute() or not audio.is_file():
        raise ValueError("Audiodatei muss absolut und lokal vorhanden sein.")
    if not modell.is_absolute() or not modell.is_dir():
        raise ValueError("ASR-Modell muss ein absoluter lokaler Modellordner sein.")
    return audio, modell


def transkribieren(anfrage: dict[str, Any]) -> dict[str, Any]:
    """
    Transkribiert eine lokale Datei mit einem bereits installierten faster-whisper-Modell.

    Args:
        anfrage: JSON-Request mit lokalen Pfaden, optionaler Sprache und Wortzeitstempeln.

    Returns:
        Objekt mit Worttext, Start-/Endzeit, Konfidenz und erkannter Sprache.

    Raises:
        RuntimeError: Wenn faster-whisper fehlt, das Modell/Audio nicht lesbar ist
            oder keine gültigen Wortsegmente ausgegeben werden.
    """
    audio_pfad, modell_pfad = lokale_pfade_pruefen(anfrage)
    try:
        from faster_whisper import WhisperModel
    except ImportError as fehler:
        raise RuntimeError(
            "faster-whisper ist in der lokalen Python-Umgebung nicht installiert. "
            "Es wurde kein Netzwerk- oder Cloud-Fallback verwendet."
        ) from fehler

    modell = WhisperModel(str(modell_pfad), device="cpu", compute_type="int8")
    sprache = anfrage.get("language")
    if not isinstance(sprache, str) or sprache.lower() in ("", "auto"):
        sprache = None
    segmente, metadaten = modell.transcribe(
        str(audio_pfad),
        language=sprache,
        word_timestamps=True,
        vad_filter=False,
        beam_size=5,
    )
    woerter: list[dict[str, Any]] = []
    for segment in segmente:
        for wort in segment.words or []:
            text = str(wort.word).strip()
            start = float(wort.start)
            ende = float(wort.end)
            wahrscheinlichkeit = float(wort.probability)
            if text and 0.0 <= start < ende:
                woerter.append(
                    {
                        "text": text,
                        "startSeconds": start,
                        "endSeconds": ende,
                        "confidence": min(1.0, max(0.0, wahrscheinlichkeit)),
                    }
                )
    if not woerter:
        raise RuntimeError("faster-whisper hat keine gültigen Wort-Zeitstempel erzeugt.")
    return {"ok": True, "language": getattr(metadaten, "language", None), "words": woerter}


def main() -> int:
    """
    Führt einen isolierten Einzelauftrag aus und hält Fehler im Workerprotokoll sichtbar.

    Args:
        Keine; liest stdin und schreibt eine JSON-Antwort auf stdout.

    Returns:
        Prozesscode null bei einem protokollierten Ergebnis; sonst Prozessfehlercode.

    Raises:
        Keine Workerfehler nach außen; sie werden in der Antwort mit verständlichem
        Fehlertext zurückgegeben, damit die Rust-App sie anzeigen und loggen kann.
    """
    try:
        antwort_schreiben(transkribieren(anfrage_lesen()))
    except Exception as fehler:  # Der äußere Prozessrand muss alle Workerfehler serialisieren.
        antwort_schreiben({"ok": False, "error": f"Lokale Lyrics-Transkription fehlgeschlagen: {fehler}"})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
