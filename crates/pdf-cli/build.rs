#[allow(dead_code)]
#[path = "../pdf-desktop/build.rs"]
mod resources;

fn main() {
    resources::emit(
        "PanPDF command line",
        "panpdf-cli.exe",
        "PanPDF.CommandLine",
    );
}
