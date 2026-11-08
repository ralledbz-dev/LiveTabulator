# Lokales LLM-Modell

## Festes Assistant-Standardmodell

Die App verwendet **Qwen3.5-4B Agentic Coder v4 (Q4_K_M)** als einziges app-lokales Assistant-Standardmodell: [GGUF-Modellkarte](https://huggingface.co/mradermacher/qwen3.5-4b-agentic-coder-v4-GGUF) · [Quell-Finetune](https://huggingface.co/mahiatlinux/qwen3.5-4b-agentic-coder-v4). Die Lizenz ist Apache-2.0. Die Datei liegt unter `Qwen3.5-4B-Q4_K_M.gguf` und hat 2.783.447.520 Bytes. SHA-256: `C1268EF0EC3B0316EA62C081645D7BBAF80CF76288E4F6A590C22A073D64C920`.

Die App-Datei wurde bytegenau aus `D:\Models\ModelVault\models\agentic-coding\qwen3.gguf` kopiert. Die Core-Registry wählt nur dieses exakt benannte, validierte GGUF als Assistant-Default. Weitere erkannte Sprachmodelle sind kein stiller Ersatz. Das frühere optionale Qwen3.5-0.8B-Testmodell wurde nach erfolgreicher 4B-Verifikation aus dem App-Modellordner entfernt.

## Gemessene lokale Leistung auf RAZOR

Auf der NVIDIA GeForce GTX 1650 SUPER mit 4 GiB VRAM wurde derselbe 32-Token-Test mit 8 CPU-Threads gefahren:

| Profil | Tokens/s |
|---|---:|
| CPU-only | 1,82 |
| CUDA, 8 Layer | 3,30 |
| CUDA, 16 Layer | 4,98 |
| CUDA, 24 Layer | 7,95 |
| CUDA, 28 Layer | 10,92 |

28 GPU-Layer waren der schnellste getestete Wert, ließen unter den gemessenen Startbedingungen aber nur rund 605 MiB VRAM frei. 24 Layer erreichten 7,95 Tokens/s und ließen rechnerisch rund 866 MiB frei. Der portable CUDA-Code-Standard bleibt deshalb bei 8 Layern; die Messwerte wurden über `LIVETAB_GPU_LAYERS` eingestellt. Eine automatische VRAM-Erkennung und adaptive Auswahl gibt es noch nicht.

Die Inferenz-Smokes testen Laden und Token-Ausgabe, nicht die semantische Qualität der Musikantwort. Ein 28-Layer-Smoke nach dem letzten Build brauchte 12,024 s zum Laden; frühere Reloads lagen bei 2,663 s. Cache-/Kaltstartbedingungen wurden nicht kontrolliert. Die 32-Token-Raten sind deshalb nicht mit einer Kaltstart- oder Audio-Latenz gleichzusetzen.

## Git und Paketierung

Große Modellgewichte werden durch `.gitignore` von Commits ausgeschlossen. Für ein portables Windows-Release muss die feste 4B-Datei anhand ihrer SHA-256 geprüft und zusammen mit `manifest.json` in `models/llm/` neben die ausführbare Datei kopiert werden. Die passenden llama.cpp-/ggml- und CUDA-Laufzeit-DLLs sind noch nicht in ein Releasepaket integriert. Die Messungen erfolgten lokal mit einer installierten CUDA-Toolchain und belegen noch keine portable Auslieferung.
