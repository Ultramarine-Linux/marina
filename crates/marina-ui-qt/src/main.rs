use qtbridge::QApp;

fn main() {
    QApp::new()
        .load_qml(include_bytes!("../qml/main.qml"))
        .run();
}
