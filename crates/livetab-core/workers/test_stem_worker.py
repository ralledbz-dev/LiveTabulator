# =============================================================================
# Division By Zeros (DBZS)
# Projekt: DBZS LiveTabulator
# Datei: test_stem_worker.py
# Zweck: Offline-Tests für Validierung, Stem-Mapping und Download-Einwilligung.
# Sprache: Python
# =============================================================================
from __future__ import annotations

import importlib.util
import sys
import tempfile
import types
import unittest
import wave
from pathlib import Path
from unittest.mock import patch

SPEZ = importlib.util.spec_from_file_location(
    "stem_worker", Path(__file__).with_name("stem_worker.py")
)
assert SPEZ and SPEZ.loader
worker = importlib.util.module_from_spec(SPEZ)
SPEZ.loader.exec_module(worker)


def wav(pfad: Path, dauer: float = 1.0) -> None:
    """Zweck: Erzeugt eine kleine Stereo-PCM-WAV-Testdatei.

    Eingaben: `pfad` – Zieldatei; `dauer` – gewünschte Dauer in Sekunden.
    Ausgabe: `None`; schreibt eine 8-kHz-/16-Bit-Test-WAV unter `pfad`.
    """
    with wave.open(str(pfad), "wb") as datei:
        datei.setnchannels(2)
        datei.setsampwidth(2)
        datei.setframerate(8000)
        datei.writeframes(b"\0\0\0\0" * int(8000 * dauer))


def fake_separator(output_factory, load_error=None):
    """Zweck: Stellt einen deterministischen Separator mit messbarem Downloader bereit.

    Eingaben: `output_factory` – Funktion für Test-WAV-Ausgaben; `load_error` – optionale Ladeausnahme.
    Ausgabe: Separator-Klasse und temporäre Importmodule für `audio-separator`.
    """

    class FakeSeparator:
        version = "fake-1"
        downloads = 0

        def __init__(self, model_file_dir, output_dir, output_format):
            """Zweck: Speichert Testpfade wie der reale Fork-Konstruktor.

            Eingaben: Modellordner, Ausgabeordner und angefordertes Audioformat.
            Ausgabe: Initialisierte Attrappe; führt keine Datei- oder Netzoperation aus.
            """
            self.model_file_dir, self.output_dir = model_file_dir, output_dir

        def download_file_if_not_exists(self, *args, **kwargs):
            """Zweck: Zählt simulierte Einzeldatei-Downloads.

            Eingaben: Beliebige Fork-Downloadargumente.
            Ausgabe: `None`; erhöht nur den Testzähler.
            """
            FakeSeparator.downloads += 1

        def download_file_from_first_available(self, *args, **kwargs):
            """Zweck: Zählt simulierte Mirror-Downloads.

            Eingaben: Beliebige Fork-Mirrorargumente.
            Ausgabe: `None`; erhöht nur den Testzähler.
            """
            FakeSeparator.downloads += 1

        def load_model(self, model_filename):
            """Zweck: Simuliert Laden oder Fehler des angeforderten Modells.

            Eingaben: Einfacher Modellname.
            Ausgabe: `None` bei Erfolg oder die beim Erzeugen gesetzte Testausnahme.
            """
            if load_error:
                raise load_error

        def separate(self, input_path):
            """Zweck: Erzeugt simulierte Separatorausgaben.

            Eingaben: Lokaler Eingabepfad.
            Ausgabe: Dateiliste aus `output_factory` im Testausgabeordner.
            """
            return output_factory(Path(self.output_dir))

    paket = types.ModuleType("audio_separator")
    submodul = types.ModuleType("audio_separator.separator")
    submodul.Separator = FakeSeparator
    paket.separator = submodul
    return FakeSeparator, {"audio_separator": paket, "audio_separator.separator": submodul}


class StemWorkerTests(unittest.TestCase):
    """Isoliert Workerpfade mit temporären WAVs und einem Mockseparator."""

    def setUp(self):
        """Zweck: Richtet eine eigene Testumgebung für jeden Test ein.

        Eingaben: Keine.
        Ausgabe: Temporäre Eingabe-WAV und Modell-/Ausgabeordner in `self`.
        """
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.eingabe = self.root / "input.wav"
        wav(self.eingabe)
        self.modelle, self.ausgabe = self.root / "models", self.root / "output"
        self.modelle.mkdir()
        self.ausgabe.mkdir()

    def tearDown(self):
        """Zweck: Entfernt alle temporären Testdateien.

        Eingaben: Keine.
        Ausgabe: `None`; das pro Test angelegte Verzeichnis ist gelöscht.
        """
        self.temp.cleanup()

    def request(self, **extra):
        """Zweck: Baut einen gültigen Workerrequest mit optionalen Testüberschreibungen.

        Eingaben: Zusätzliche JSON-Felder als Schlüsselwortargumente.
        Ausgabe: Request-Dictionary mit absoluten Eingabe-, Modell- und Ausgabepfaden.
        """
        wert = {
            "inputPath": str(self.eingabe),
            "modelDirectory": str(self.modelle),
            "outputDirectory": str(self.ausgabe),
        }
        wert.update(extra)
        return wert

    def test_erfolgreiches_vier_stem_mapping(self):
        """Zweck: Prüft Mapping und Metadaten der vier erwarteten Stem-Ausgaben.

        Eingaben: Keine; nutzt einen Mockseparator mit vier gültigen WAV-Dateien.
        Ausgabe: Keine; Assertions verlangen vier Klassen und korrekte Formatmetadaten.
        """

        def outputs(root):
            """Zweck: Erzeugt vier gültige Mock-Stems.

            Eingaben: Jobbezogener Ausgabeordner.
            Ausgabe: Liste der erzeugten WAV-Pfade.
            """
            result = []
            for label in ("Vocals", "Drums", "Bass", "Other"):
                pfad = root / f"song ({label}).wav"
                wav(pfad)
                result.append(str(pfad))
            return result

        _, modules = fake_separator(outputs)
        with patch.dict(sys.modules, modules):
            ereignisse = worker.verarbeiten(self.request(allowModelDownload=True))
        ergebnis = next(e for e in ereignisse if e["event"] == "result")
        self.assertEqual(
            [s["kind"] for s in ergebnis["stems"]],
            ["vocals", "drums", "bass", "other"],
        )
        self.assertTrue(
            all(s["sampleRate"] == 8000 and s["channels"] == 2 for s in ergebnis["stems"])
        )

    def test_fehlendes_modell_ohne_download(self):
        """Zweck: Prüft, dass ein fehlendes Basismodell nur einen Freigabebedarf meldet.

        Eingaben: Keine; der Modellordner ist leer und die Freigabe fehlt.
        Ausgabe: Keine; `needs_download` wird gemeldet und kein Downloader aufgerufen.
        """
        fake, modules = fake_separator(lambda root: [])
        with patch.dict(sys.modules, modules):
            ereignisse = worker.verarbeiten(self.request())
        fehlendes_modell = next(e for e in ereignisse if e["event"] == "needs_download")
        self.assertEqual(fehlendes_modell["missing"], ["htdemucs.yaml"])
        self.assertEqual(fake.downloads, 0)

    def test_freigabe_umgeht_neue_needs_download_antwort(self):
        """Zweck: Prüft, dass der Worker bei gesetzter Zustimmung den Downloadpfad betritt.

        Eingaben: Keine; das Mockmodell fehlt, die Freigabe ist aber gesetzt.
        Ausgabe: Keine; der Worker meldet keinen erneuten Zustimmungsbedarf.
        """
        _, modules = fake_separator(lambda root: [])
        with patch.dict(sys.modules, modules):
            ereignisse = worker.verarbeiten(self.request(allowModelDownload=True))
        self.assertTrue(any(e["event"] == "error" for e in ereignisse))
        self.assertNotIn("needs_download", {e["event"] for e in ereignisse})

    def test_download_gesperrt_bis_ausdrueckliche_freigabe(self):
        """Zweck: Prüft Blockierung und anschließende Freigabe eines fehlenden Gewichtdownloads.

        Eingaben: Lokale Modellkonfiguration und Mockseparator, der ein Gewicht anfordert.
        Ausgabe: Keine; Ablehnung blockiert Netzwerkpfade, Zustimmung lässt einen Downloadpfad zu.
        """

        def outputs(root):
            """Zweck: Erzeugt vier WAV-Ausgaben für den freigegebenen Mocklauf.

            Eingaben: Jobbezogener Ausgabeordner.
            Ausgabe: Liste der erzeugten WAV-Pfade.
            """
            result = []
            for label in ("Vocals", "Drums", "Bass", "Other"):
                pfad = root / f"song ({label}).wav"
                wav(pfad)
                result.append(str(pfad))
            return result

        fake, modules = fake_separator(outputs)
        (self.modelle / "htdemucs.yaml").write_text(
            "lokale Modellkonfiguration", encoding="utf-8"
        )

        def load_model_with_missing_weight(separator, model_filename):
            """Zweck: Simuliert ein fehlendes Modellgewicht während des Ladens.

            Eingaben: Separatorattrappe und angeforderter Modellname.
            Ausgabe: `None` nach Weiterleitung an den Downloader der Attrappe.
            """
            separator.download_file_if_not_exists(
                "https://example.invalid/weights.pth", "weights.pth"
            )

        fake.load_model = load_model_with_missing_weight
        with patch.dict(sys.modules, modules):
            denied = worker.verarbeiten(self.request())
            missing = next(e for e in denied if e["event"] == "needs_download")
            self.assertIn("weights.pth", missing["missing"])
            self.assertEqual(fake.downloads, 0)

            allowed = worker.verarbeiten(self.request(allowModelDownload=True))
            self.assertTrue(any(e["event"] == "result" for e in allowed))
            self.assertEqual(fake.downloads, 1)

    def test_ungueltige_pfade_und_output_escape(self):
        """Zweck: Verwirft relative Eingaben und Separatorausgaben außerhalb des Zielordners.

        Eingaben: Keine; beide Fehlerfälle werden mit einer Separatorattrappe ausgelöst.
        Ausgabe: Keine; beide Aufrufe müssen mindestens ein Fehlerereignis liefern.
        """
        _, modules = fake_separator(lambda root: [str(self.root / "escape (Vocals).wav")])
        with patch.dict(sys.modules, modules):
            self.assertTrue(
                any(
                    e["event"] == "error"
                    for e in worker.verarbeiten(self.request(inputPath="relative.wav"))
                )
            )
            self.assertTrue(
                any(
                    e["event"] == "error"
                    for e in worker.verarbeiten(self.request(allowModelDownload=True))
                )
            )

    def test_malformed_und_unaligned_outputs(self):
        """Zweck: Verwirft beschädigte WAV-Ausgaben und zeitlich verschobene Stems.

        Eingaben: Keine; nutzt eine kaputte WAV sowie eine gültige WAV mit falscher Dauer.
        Ausgabe: Keine; beide Fälle müssen ein verständliches Fehlerereignis enthalten.
        """

        def bad(root):
            """Zweck: Erzeugt eine beschädigte Testdatei mit gültigem Stemlabel.

            Eingaben: Jobbezogener Ausgabeordner.
            Ausgabe: Liste aus einem Pfad, dessen Datei keine WAV ist.
            """
            pfad = root / "song (Vocals).wav"
            pfad.write_bytes(b"not wav")
            return [str(pfad)]

        _, modules = fake_separator(bad)
        with patch.dict(sys.modules, modules):
            self.assertTrue(
                any(
                    e["event"] == "error"
                    for e in worker.verarbeiten(self.request(allowModelDownload=True))
                )
            )

        def unaligned(root):
            """Zweck: Erzeugt eine gültige, aber zu lange Stem-WAV.

            Eingaben: Jobbezogener Ausgabeordner.
            Ausgabe: Liste mit einem Stem, dessen Dauer vom Eingang abweicht.
            """
            pfad = root / "song (Vocals).wav"
            wav(pfad, 1.1)
            return [str(pfad)]

        _, modules = fake_separator(unaligned)
        with patch.dict(sys.modules, modules):
            ereignisse = worker.verarbeiten(self.request(allowModelDownload=True))
        self.assertTrue(
            any(e["event"] == "error" and "zeitlich" in e["message"] for e in ereignisse)
        )


if __name__ == "__main__":
    unittest.main()
