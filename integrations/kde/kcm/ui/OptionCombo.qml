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
    onActivated: kcm.settings.set(key, shown[currentIndex].value)
}
