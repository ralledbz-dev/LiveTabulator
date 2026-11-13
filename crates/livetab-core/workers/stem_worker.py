# =============================================================================
# Division By Zeros (DBZS)
# Projekt: DBZS LiveTabulator
# Datei: stem_worker.py
# Zweck: Lokale Vier-Stem-Trennung über den gepinnten audio-separator-Fork.
# Sprache: Python
# Hinweise:
# - Ausschließlich absolute lokale WAV-, Modell- und Ausgabeordner akzeptieren.
# - Modell-Downloads sind standardmäßig vollständig gesperrt und benötigen Einwilligung.
# - Diagnosen gehören auf stderr; stdout enthält ausschließlich JSON-Lines-Ereignisse.
# =============================================================================
from __future__ import annotations

import json
import re
import sys
import wave
from pathlib import Path
from typing import Any

ERLAUBTE_STEMS = ("vocals", "drums", "bass", "other")
MAX_AUSGABEBYTES = 2_000_000_000
SOURCE_CONTEXT = {
    "mirror": "https://github.com/nomadkaraoke/python-audio-separator/releases/download/model-configs",
    "upstream": "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models; https://raw.githubusercontent.com/TRvlvr/application_data/main/filelists/download_checks.json; Demucs-Gewichte gemäß htdemucs.yaml (ggf. dl.fbaipublicfiles.com/demucs/...)",
    "host": "GitHub, raw.githubusercontent.com und der vom Demucs-Modell referenzierte Host (gegebenenfalls Meta/Facebook)",
}


class FehlendesModell(RuntimeError):
    """Fehlerzustand für fehlende lokale Modellressourcen oder gesperrte Downloader."""

    def __init__(self, fehlend: list[str]):
        """Zweck: Erzeugt eine eindeutige Liste fehlender Modellressourcen.

        Eingaben: `fehlend` – Dateinamen oder Ressourcennamen, die der Worker nicht lokal fand.
        Ausgabe: Initialisierte Ausnahme mit sortierter, duplikatfreier Ressourcenliste.
        """
        self.fehlend = sorted(set(fehlend)) or ["unbekannte Modellressource"]
        super().__init__("Modellressource fehlt: " + ", ".join(self.fehlend))


def ereignis_schreiben(ereignis: dict[str, Any]) -> None:
    """Zweck: Sendet ein Workerereignis als JSON-Line an den Rust-Prozess.

    Eingaben: `ereignis` – JSON-kompatibles Ereignisobjekt.
    Ausgabe: `None`; schreibt eine Zeile auf stdout und leert den Puffer.
    """
    sys.stdout.write(json.dumps(ereignis, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def _pfad(anfrage: dict[str, Any], feld: str, ordner: bool = False) -> Path:
    """Zweck: Validiert einen absoluten lokalen Pfad aus dem Workerrequest.

    Eingaben: Requestdictionary, Feldname und optional die Forderung nach einem echten Ordner.
    Ausgabe: Aufgelöster `Path`-Wert oder eine konkrete Validierungsausnahme.
    """
    wert = anfrage.get(feld)
    if not isinstance(wert, str) or not wert:
        raise ValueError(f"Pflichtfeld {feld} fehlt.")
    pfad = Path(wert).expanduser()
    if not pfad.is_absolute():
        raise ValueError(f"{feld} muss absolut sein.")
    if ordner and (pfad.is_symlink() or not pfad.is_dir()):
        raise ValueError(f"{feld} muss ein vorhandener, echter Ordner sein.")
    return pfad


def anfrage_pruefen(anfrage: dict[str, Any]) -> tuple[Path, Path, Path, str, bool]:
    """Zweck: Validiert Eingabe-WAV, Ordner, Modellname und Downloadfreigabe.

    Eingaben: JSON-Request des Rust-Aufrufers.
    Ausgabe: Eingabe-, Modell- und Ausgabeordner, Modellname sowie Freigabestatus.
    """
    eingabe = _pfad(anfrage, "inputPath")
    modelle = _pfad(anfrage, "modelDirectory", True)
    ausgabe = _pfad(anfrage, "outputDirectory", True)
    if eingabe.is_symlink() or not eingabe.is_file() or eingabe.suffix.lower() != ".wav":
        raise ValueError("inputPath muss eine vorhandene, echte WAV-Datei sein.")
    modellname = anfrage.get("modelFilename", "htdemucs.yaml")
    if not isinstance(modellname, str) or not modellname or Path(modellname).name != modellname:
        raise ValueError("modelFilename muss ein einzelner Dateiname sein.")
    erlauben = anfrage.get("allowModelDownload", False)
    if not isinstance(erlauben, bool):
        raise ValueError("allowModelDownload muss boolean sein.")
    return eingabe, modelle, ausgabe, modellname, erlauben


def _wav_metadaten(pfad: Path) -> tuple[int, int, float]:
    """Zweck: Liest WAV-Format und Dauer ohne externe Audioabhängigkeit.

    Eingaben: Pfad einer WAV-Datei.
    Ausgabe: Sample-Rate, Kanalzahl und Dauer in Sekunden oder eine Formatfehlermeldung.
    """
    try:
        with wave.open(str(pfad), "rb") as wav:
            rate, kanaele, frames = wav.getframerate(), wav.getnchannels(), wav.getnframes()
    except (OSError, wave.Error) as fehler:
        raise ValueError(f"Ungültige WAV-Datei {pfad.name}: {fehler}") from fehler
    if rate <= 0 or kanaele <= 0 or frames <= 0:
        raise ValueError(f"WAV-Datei {pfad.name} enthält keine gültigen Metadaten.")
    return rate, kanaele, frames / rate


def _label(pfad: Path) -> str | None:
    """Zweck: Übersetzt den Klammerzusatz eines Separator-Dateinamens in eine Stemklasse.

    Eingaben: Ausgabepfad mit einem Label wie `(Vocals)`, `(Bass)` oder `(Other)`.
    Ausgabe: Erlaubter Schlüssel oder `None`, falls der Dateiname kein bekanntes Stemlabel trägt.
    """
    treffer = re.findall(r"\(([^()]*)\)", pfad.stem)
    if not treffer:
        return None
    wert = re.sub(r"[^a-z]", "", treffer[-1].lower())
    return {
        "vocal": "vocals",
        "vocals": "vocals",
        "drum": "drums",
        "drums": "drums",
        "bass": "bass",
        "other": "other",
        "instrumental": "other",
    }.get(wert)


def _sicherheitsdownload_sperren(separator: Any) -> None:
    """Zweck: Sperrt beide Fork-Downloader, solange kein Nutzer zugestimmt hat.

    Eingaben: Initialisiertes Separatorobjekt aus dem gepinnten Fork.
    Ausgabe: `None`; ersetzt Downloadmethoden durch eine Meldung fehlender Ressourcen.
    """

    def blockiert(*args: Any, **kwargs: Any) -> None:
        """Zweck: Wandelt einen Downloadversuch in einen Freigabebedarf um.

        Eingaben: Forkargumente; optionaler Zieldateiname in `output_path` oder Position 2.
        Ausgabe: Kein Rückgabewert; löst `FehlendesModell` statt eines Netzabrufs aus.
        """
        ziel = kwargs.get("output_path") or (args[1] if len(args) > 1 else "unbekannt")
        raise FehlendesModell([Path(str(ziel)).name])

    separator.download_file_if_not_exists = blockiert
    separator.download_file_from_first_available = blockiert


def _needs_download(modellname: str, fehlend: list[str]) -> dict[str, Any]:
    """Zweck: Erzeugt die Freigabeantwort für nicht lokal verfügbare Modelle.

    Eingaben: Angeforderter Modellname und fehlende Dateinamen.
    Ausgabe: JSON-kompatibles Ereignis mit Quellenkontext und nicht geprüfter Lizenzangabe.
    """
    return {
        "event": "needs_download",
        "model": modellname,
        "missing": sorted(set(fehlend)),
        "sourceContext": SOURCE_CONTEXT,
        "licenseStatus": "unverified by app",
    }


def trennen(anfrage: dict[str, Any]) -> list[dict[str, Any]]:
    """Zweck: Führt eine lokale Vier-Stem-Trennung aus und prüft alle Separatorausgaben.

    Eingaben: Validierter oder zu validierender Workerrequest mit WAV- und Modellpfaden.
    Ausgabe: Vier überprüfte Stemobjekte mit Pfad, Format, Dauer und Modellherkunft.
    """
    eingabe, modelle, ausgabe, modellname, erlauben = anfrage_pruefen(anfrage)
    _, _, eingabedauer = _wav_metadaten(eingabe)
    if not erlauben and not (modelle / modellname).is_file():
        raise FehlendesModell([modellname])
    try:
        from audio_separator.separator import Separator
    except ImportError as fehler:
        raise RuntimeError("audio-separator ist in der lokalen Python-Umgebung nicht installiert.") from fehler
    separator = Separator(model_file_dir=str(modelle), output_dir=str(ausgabe), output_format="WAV")
    if not erlauben:
        _sicherheitsdownload_sperren(separator)
    try:
        separator.load_model(model_filename=modellname)
        erzeugt = separator.separate(str(eingabe))
    except FehlendesModell:
        raise
    except (FileNotFoundError, OSError) as fehler:
        if not erlauben:
            raise FehlendesModell([modellname]) from fehler
        raise
    if not isinstance(erzeugt, (list, tuple)):
        raise ValueError("Separator lieferte keine Dateiliste.")
    gefundene: dict[str, Path] = {}
    root = ausgabe.resolve()
    for element in erzeugt:
        pfad = Path(str(element))
        if not pfad.is_absolute():
            pfad = ausgabe / pfad
        label = _label(pfad)
        if label is None:
            raise ValueError(f"Ausgabedatei ohne geplantes Stemlabel in Klammern: {pfad.name}")
        if label in gefundene:
            raise ValueError(f"Stem {label} wurde mehrfach ausgegeben.")
        real = pfad.resolve()
        if pfad.is_symlink() or not pfad.is_file() or root not in real.parents:
            raise ValueError(f"Stem-Ausgabe liegt außerhalb des Ausgabeordners: {pfad}")
        if pfad.stat().st_size <= 0 or pfad.stat().st_size > MAX_AUSGABEBYTES:
            raise ValueError(f"Stem-Ausgabe hat eine unzulässige Größe: {pfad.name}")
        rate, kanaele, dauer = _wav_metadaten(pfad)
        if abs(dauer - eingabedauer) > 0.020:
            raise ValueError(f"Stem {label} ist nicht zeitlich an die Eingabe ausgerichtet.")
        gefundene[label] = pfad
    if set(gefundene) != set(ERLAUBTE_STEMS):
        raise ValueError("Separator muss genau vocals, drums, bass und other liefern.")
    version = getattr(separator, "version", None)
    if version is None:
        try:
            from importlib.metadata import version as paketversion

            version = paketversion("audio-separator")
        except Exception:
            version = None
    return [
        {
            "kind": label,
            "path": str(gefundene[label]),
            "sampleRate": _wav_metadaten(gefundene[label])[0],
            "channels": _wav_metadaten(gefundene[label])[1],
            "durationSeconds": _wav_metadaten(gefundene[label])[2],
            "modelFilename": modellname,
            "modelVersion": version,
        }
        for label in ERLAUBTE_STEMS
    ]


def verarbeiten_stream(anfrage: dict[str, Any]):
    """Zweck: Verarbeitet einen Request und streamt JSON-Lines-Fortschritt ungepuffert.

    Eingaben: JSON-kompatibler Workerrequest.
    Ausgabe: Ereignisiterator für Fortschritt, Ergebnis, Downloadfreigabe oder Fehler.
    """
    yield {"event": "progress", "stage": "validating", "percent": 0}
    try:
        anfrage_pruefen(anfrage)
        yield {"event": "progress", "stage": "separating", "percent": 25}
        stems = trennen(anfrage)
        yield {"event": "progress", "stage": "complete", "percent": 100}
        yield {"event": "result", "stems": stems}
    except FehlendesModell as fehler:
        yield _needs_download(str(anfrage.get("modelFilename", "htdemucs.yaml")), fehler.fehlend)
    except Exception as fehler:
        yield {"event": "error", "message": str(fehler)}


def verarbeiten(anfrage: dict[str, Any]) -> list[dict[str, Any]]:
    """Zweck: Sammelt gestreamte Workerereignisse für Offline-Tests und synchrone Aufrufer.

    Eingaben: JSON-kompatibler Workerrequest.
    Ausgabe: Vollständige Liste aus Fortschritts-, Ergebnis-, Freigabe- oder Fehlerereignissen.
    """
    return list(verarbeiten_stream(anfrage))


def main() -> int:
    """Zweck: Betreibt den Worker als JSON-Lines-Unterprozess für die Rust-App.

    Eingaben: Ein JSON-Request von stdin.
    Ausgabe: Exitcode `0`; Ereignisse gehen auf stdout, Diagnosen auf stderr.
    """
    try:
        roh = sys.stdin.read()
        anfrage = json.loads(roh)
        if not isinstance(anfrage, dict):
            raise ValueError("Anfrage muss ein JSON-Objekt sein.")
        for ereignis in verarbeiten_stream(anfrage):
            ereignis_schreiben(ereignis)
    except Exception as fehler:
        print(f"Stem-Worker: {fehler}", file=sys.stderr)
        ereignis_schreiben({"event": "error", "message": str(fehler)})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
