slint::slint! {
    export component MainWindow inherits Window {
        width: 1280px;
        height: 720px;
        title: "Marina";
        background: #1f2430;

        Text {
            text: "Marina";
            color: #e5e9f0;
            font-size: 36px;
            horizontal-alignment: center;
            vertical-alignment: center;
            width: parent.width;
            height: parent.height;
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    MainWindow::new()?.run()
}
