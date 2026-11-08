/// Zweck: Übersetzt das Slint-Markup beim Cargo-Build in Rust-UI-Code.
/// Eingaben: Cargo-Buildumgebung und die Datei `ui/main.slint`.
/// Ausgabe: Keine Rückgabe; erzeugt Buildartefakte oder bricht bei einem Slint-Fehler ab.
fn main() {
    slint_build::compile("ui/main.slint").expect("Slint UI compilation failed");
}
