# Quickshell Integration

This integration shows a floating mic icon in the center of the screen while hyprwhspr is recording.

https://github.com/user-attachments/assets/62d51e4c-1c3f-4774-b33e-5539dc56c91e

![a mic icon centered in the screen, indicating hyprwhspr is recording](./demo/mic.png)

## Integration implementation
The way it knows when hyprwhspr is recording is by watching `~/.cache/hyprwhspr-rs/status.json` with a `FileView`
```qml
FileView {
    path: `${Quickshell.env("HOME")}/.cache/hyprwhspr-rs/status.json`
    watchChanges: true
    onFileChanged: reload()
    JsonAdapter {
        property string alt: ""
        onAltChanged: statusIndicator.visible = alt === "active"
    }
}
```

## Integration usage

Put `mic.svg` and `HyprwhsprStatus.qml` in `~/.config/quickshell/hyprwhsprStatus` and edit your `~/.config/quickshell/shell.qml` to include
```qml
import qs.hyprwhsprStatus

Scope {
    // your other quickshell stuff
    HyprwhsprStatus {}
}
```
