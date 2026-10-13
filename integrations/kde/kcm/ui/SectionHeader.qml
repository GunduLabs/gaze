// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// A group title with a line under it, as the headings on the GNOME page.
ColumnLayout {
    property alias text: heading.text

    Layout.fillWidth: true
    Layout.topMargin: Kirigami.Units.largeSpacing
    spacing: Kirigami.Units.smallSpacing

    Kirigami.Heading {
        id: heading
        level: 3
    }
    Kirigami.Separator {
        Layout.fillWidth: true
    }
}
