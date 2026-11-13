# Windows-x64-Crossbuild für den ASIO-Test — Stand 2026-11-13

## Ergebnis

Der vollständige Windows-GNU-Zielcheck und der App-Link sind jetzt gelungen:

- `cargo check --locked --target x86_64-pc-windows-gnu -p livetab-app` — bestanden.
- `cargo build --locked --target x86_64-pc-windows-gnu -p livetab-app` — bestanden; erzeugte `livetab-app.exe` ist ein PE32+-Programm für x86-64-Windows.

Damit wurden der Windows-ASIO-Adapter, das WASAPI-Routing und die nativen `llama.cpp`-Bibliotheken im vollständigen App-Build durchlaufen. Der Rust-Build meldete drei vorhandene Dead-Code-Warnungen in Aufnahme-/Playbackmodulen, aber keine Buildfehler.

## Ursache und Build-Entscheidung

Der anfängliche `stdbool.h`-Fehler entstand in Bindgen/Clang, weil beim Linux-Crossbuild die MinGW-Target-Header nicht vollständig in Clangs Suchpfad lagen. Die gepinnte `llama-cpp-sys-4`-Version 0.11.0 wurde anhand ihres lokalen Registry-`build.rs` geprüft: Sie übergibt `CC`/`CXX` an CMake und verlangt beim Windows-GNU-Link zusätzlich statisches `stdc++` sowie `winpthread`.

Für diesen Linux-Sandboxlauf wurden daher temporär explizite Clang-Sysroot-/Headerpfade und `CMAKE_SYSROOT` gesetzt. Die installierten MinGW-w64-11.0.1-Header enthielten außerdem die vom gepinnten llama.cpp-Quellstand verwendeten Thread-Power-Throttling-Deklarationen nicht; ein nur in `.work` abgelegter Kompatibilitätsheader wurde über temporäre C/C++-Wrapper vorgeladen. Für den letzten Rust-Link wurden die vorhandenen MinGW-`libstdc++`- und `winpthread`-Suchordner ausschließlich für das Windows-Target angegeben.

Das sind sandbox-/toolchain-spezifische Build-Overrides, keine portable Projekt-Buildkonfiguration. Die Buildkorrektur hat keine App-Quelldateien, `Cargo.toml` oder `Cargo.lock` geändert. Es wurde kein zusätzlicher Commit erstellt und nichts gepusht.

## Testpaket

Das geprüfte Paket liegt unter `.work/windows-asio-test-x64-gnu.zip` (38.308.408 Bytes). Es enthält eine gestrippte, nicht Release-optimierte Windows-x64-App, die benötigten llama/ggml-DLLs, MinGW-Runtime-DLLs sowie eine deutsche `README.txt`. ZIP-Integrität und die nicht-systemeigenen PE-DLL-Imports wurden geprüft; für alle ermittelten Imports sind Dateien im Paket enthalten.

Für den Hardwaretest unter Windows muss das Verzeichnis vollständig entpackt werden; die DLLs müssen neben `livetab-app.exe` bleiben. Der ASIO-Treiber des Interfaces muss auf dem Windows-PC installiert sein. In der Oberfläche auf „Quellen“ klicken, den ASIO-Eingang auswählen und „Live starten“ drücken. WASAPI bleibt der Systemmix; ASIO ist der direkte Interfaceeingang.

## Noch offen

Die App konnte hier nicht unter Windows gestartet werden. Treiberscan, echte ASIO-Aufnahme, Audioqualität, Bufferverluste und Latenz sind daher weiterhin ungeprüft und benötigen ein Windows-Gerät mit installiertem ASIO-Treiber.

## Verwendete Quellen

- `Cargo.lock` und die lokale Registry-Quelle `llama-cpp-sys-4` 0.11.0 (`build.rs`) für den tatsächlichen Bindgen-, CMake- und Linkpfad.
- Lokale Sandbox-Toolchain: LLVM/Clang 18, MinGW-w64-Header 11.0.1 und MinGW GCC/G++ 13.2.0 (Win32-Thread-Variante).
- Microsofts Definition von [`THREAD_POWER_THROTTLING_STATE`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-thread_power_throttling_state) sowie die MinGW-w64-Header `winbase.h`/`processthreadsapi.h`, um die fehlenden lokalen Deklarationen eng zu ergänzen.
