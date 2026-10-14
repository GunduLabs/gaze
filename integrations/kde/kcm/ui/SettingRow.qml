// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// One option: title and explanation on the left, the control at the right edge centered on both.
RowLayout {
    id: row

    property string title
    property string hint
    default property alias control: holder.data

    Layout.fillWidth: true
    spacing: Kirigami.Units.largeSpacing

    ColumnLayout {
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter
        spacing: 0
        QQC2.Label {
            Layout.fillWidth: true
            text: row.title
            wrapMode: Text.WordWrap
        }
        Hint {
            visible: row.hint.length > 0
            Layout.maximumWidth: Infinity
            text: row.hint
        }
    }
    RowLayout {
        id: holder
        Layout.alignment: Qt.AlignVCenter
    }
}
