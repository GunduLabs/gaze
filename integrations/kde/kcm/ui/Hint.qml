// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// The explanation under a setting, taken from the GNOME settings page.
QQC2.Label {
    Layout.fillWidth: true
    Layout.maximumWidth: Kirigami.Units.gridUnit * 26
    wrapMode: Text.WordWrap
    font: Kirigami.Theme.smallFont
    color: Kirigami.Theme.disabledTextColor
}
