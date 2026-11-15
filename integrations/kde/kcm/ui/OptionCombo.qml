// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2

// A combo box bound to one configuration key. Options are { text, value } pairs.
QQC2.ComboBox {
    id: root

    required property string key
    property var options: []

    readonly property var current: kcm.settings.values[key]
    // A value the daemon reports but we do not list still has to show up.
    readonly property var shown: {
        const list = options.slice();
        if (current !== undefined && !list.some(o => o.value === current)) {
            list.push({ text: String(current), value: current });
        }
        return list;
    }

    model: shown
    textRole: "text"
    valueRole: "value"
    enabled: kcm.settings.editable
    currentIndex: Math.max(0, shown.findIndex(o => o.value === current))
    property var pendingValue
    function recognizerFor(level) {
        if (level === "custom") return kcm.settings.values["security.recognizer"] || "standard";
        return level === "high" || level === "maximum" ? "accurate" : "standard";
    }
    onActivated: {
        const selected = shown[currentIndex].value;
        const changesModel = key === "security.recognizer" ? selected !== current
            : key === "security.level" && selected !== "custom" && recognizerFor(selected) !== recognizerFor(current);
        if (changesModel) {
            pendingValue = selected;
            confirmModel.open();
        } else {
            kcm.settings.set(key, selected);
        }
    }
    QQC2.Dialog {
        id: confirmModel
        title: i18n("Re-enroll faces after this change?")
        modal: true
        standardButtons: QQC2.Dialog.Ok | QQC2.Dialog.Cancel
        contentItem: QQC2.Label {
            text: i18n("Existing captures cannot match with the new recognition model. They remain on disk. After saving, use Improve this face for each profile, then test recognition.")
            wrapMode: Text.WordWrap
            implicitWidth: 360
        }
        onAccepted: kcm.settings.set(root.key, root.pendingValue)
        onRejected: root.currentIndex = Qt.binding(function() { return Math.max(0, root.shown.findIndex(o => o.value === root.current)); })
    }
}
