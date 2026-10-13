// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2

// A setting row whose control is a switch bound to one configuration key.
SettingRow {
    id: row

    property string key
    property bool allowed: true

    QQC2.Switch {
        enabled: kcm.settings.editable && row.allowed
        checked: kcm.settings.values[row.key] === true
        onToggled: kcm.settings.set(row.key, checked)
    }
}
