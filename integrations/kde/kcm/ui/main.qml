// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.ScrollViewKCM {
    id: root

    readonly property bool enrolling: kcm.enrollState === "running"
    readonly property bool testing: kcm.testState === "running"

    title: i18n("Enrolled Faces")
    implicitWidth: Kirigami.Units.gridUnit * 36
    implicitHeight: Kirigami.Units.gridUnit * 30

    actions: [
        Kirigami.Action {
            text: i18n("Configure")
            tooltip: i18n("Configure Gaze")
            icon.name: "configure"
            onTriggered: {
                // Back navigation can retain subpages and only change currentIndex.
                // Reuse Settings rather than disabling Configure or pushing duplicates.
                if (kcm.depth > 1) {
                    kcm.currentIndex = 1;
                } else {
                    kcm.push("SettingsPage.qml");
                }
            }
        },
        Kirigami.Action {
            text: i18n("Test")
            tooltip: i18n("Test face authentication")
            icon.name: "media-playback-start"
            enabled: list.count > 0 && !root.enrolling && !root.testing
            onTriggered: kcm.testFace()
        },
        Kirigami.Action {
            text: i18n("Add Face…")
            tooltip: i18n("Enroll a new face")
            icon.name: "list-add"
            enabled: !root.enrolling && !root.testing
            onTriggered: enrollDialog.openFor("")
        }
    ]

    header: ColumnLayout {
        spacing: Kirigami.Units.smallSpacing

        QQC2.Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: i18n("Faces enrolled for %1", kcm.user)
            color: Kirigami.Theme.disabledTextColor
            wrapMode: Text.WordWrap
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.error.length > 0
            type: Kirigami.MessageType.Error
            text: kcm.error
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.testState !== "idle"
            type: switch (kcm.testState) {
                case "success": return Kirigami.MessageType.Positive;
                case "running": return Kirigami.MessageType.Information;
                default: return Kirigami.MessageType.Warning;
            }
            text: switch (kcm.testState) {
                case "running": return i18n("Look at the camera…");
                case "success": return i18n("Face recognized");
                case "timeout": return i18n("No face seen before the test timed out");
                default: return i18n("Face not recognized");
            }
        }
    }

    view: ListView {
        id: list
        model: kcm.faces
        clip: true

        Kirigami.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 4
            visible: list.count === 0 && kcm.error.length === 0
            icon.name: "user-identity"
            text: i18n("No Faces Enrolled")
            explanation: i18n("Enroll a face to get started")
            helpfulAction: Kirigami.Action {
                text: i18n("Add Face…")
                icon.name: "list-add"
                enabled: !root.enrolling && !root.testing
                onTriggered: enrollDialog.openFor("")
            }
        }

        delegate: QQC2.ItemDelegate {
            id: row
            required property string name
            required property int samples
            required property bool rgb
            required property bool ir
            width: ListView.view.width
            hoverEnabled: false
            down: false

            contentItem: RowLayout {
                spacing: Kirigami.Units.smallSpacing

                Kirigami.IconTitleSubtitle {
                    Layout.fillWidth: true
                    icon.name: "user-identity"
                    title: row.name
                    subtitle: i18np("%1 capture", "%1 captures", row.samples)
                }
                Repeater {
                    model: [
                        { label: i18n("RGB"), on: row.rgb },
                        { label: i18n("IR"), on: row.ir }
                    ]
                    delegate: QQC2.Label {
                        required property var modelData
                        text: modelData.label
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        font.bold: modelData.on
                        color: modelData.on ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.disabledTextColor
                        padding: Kirigami.Units.smallSpacing
                        background: Rectangle {
                            radius: Kirigami.Units.cornerRadius
                            color: modelData.on ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.alternateBackgroundColor
                        }
                    }
                }
                QQC2.ToolButton {
                    icon.name: "edit-rename"
                    text: i18n("Rename")
                    display: QQC2.AbstractButton.IconOnly
                    enabled: !root.enrolling && !root.testing
                    onClicked: {
                        renameDialog.faceName = row.name;
                        renameDialog.open();
                    }
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                }
                QQC2.ToolButton {
                    icon.name: "camera-photo"
                    text: i18n("Improve this face")
                    display: QQC2.AbstractButton.IconOnly
                    enabled: !root.enrolling && !root.testing
                    onClicked: enrollDialog.openFor(row.name)
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                }
                QQC2.ToolButton {
                    icon.name: "edit-delete"
                    text: i18n("Delete")
                    display: QQC2.AbstractButton.IconOnly
                    enabled: !root.enrolling && !root.testing
                    onClicked: {
                        deleteDialog.faceName = row.name;
                        deleteDialog.open();
                    }
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                }
            }
        }
    }

    Kirigami.Dialog {
        id: enrollDialog

        // An empty name means a new face; otherwise the named face gets more captures.
        property string refineName: ""
        readonly property bool refining: refineName.length > 0
        readonly property bool started: kcm.enrollState !== "idle"
        readonly property string chosenName: refining ? refineName : nameField.text.trim()

        function openFor(name) {
            refineName = name;
            nameField.text = kcm.suggestedName();
            open();
        }

        title: refining ? i18n("Improve “%1”", refineName) : i18n("Add Face")
        preferredWidth: Kirigami.Units.gridUnit * 22
        closePolicy: started && kcm.enrollState === "running" ? QQC2.Popup.NoAutoClose : QQC2.Popup.CloseOnEscape
        standardButtons: Kirigami.Dialog.NoButton
        onClosed: kcm.stopEnroll()

        ColumnLayout {
            spacing: Kirigami.Units.smallSpacing

            QQC2.TextField {
                id: nameField
                Layout.fillWidth: true
                visible: !enrollDialog.refining && !enrollDialog.started
                placeholderText: i18n("Face name")
                onAccepted: if (enrollDialog.chosenName.length > 0 && !kcm.hasFace(enrollDialog.chosenName)) kcm.startEnroll(enrollDialog.chosenName)
            }
            QQC2.Label {
                Layout.fillWidth: true
                visible: nameField.visible && kcm.hasFace(nameField.text.trim())
                text: i18n("A face with this name already exists")
                color: Kirigami.Theme.negativeTextColor
                wrapMode: Text.WordWrap
            }
            QQC2.Label {
                Layout.fillWidth: true
                visible: !enrollDialog.started
                text: enrollDialog.refining
                    ? i18n("Look at the camera and follow the prompts to add more captures to “%1”.", enrollDialog.refineName)
                    : i18n("Look at the camera and follow the prompts. Keep your face centered and well lit.")
                wrapMode: Text.WordWrap
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.gridUnit * 14
                visible: enrollDialog.started
                color: "black"
                radius: Kirigami.Units.cornerRadius
                clip: true

                Image {
                    anchors.fill: parent
                    source: kcm.preview
                    fillMode: Image.PreserveAspectFit
                    cache: false
                    visible: kcm.preview.length > 0
                }
                QQC2.BusyIndicator {
                    anchors.centerIn: parent
                    running: kcm.enrollState === "running" && kcm.preview.length === 0
                }
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                visible: enrollDialog.started
                spacing: Kirigami.Units.smallSpacing

                Kirigami.Icon {
                    visible: kcm.enrollState === "done" || kcm.enrollState === "failed"
                    source: kcm.enrollState === "done" ? "emblem-ok-symbolic" : "emblem-error-symbolic"
                    color: kcm.enrollState === "done" ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                    implicitWidth: Kirigami.Units.iconSizes.smallMedium
                    implicitHeight: Kirigami.Units.iconSizes.smallMedium
                }
                QQC2.Label {
                    font.bold: true
                    text: kcm.prompt
                    wrapMode: Text.WordWrap
                    color: kcm.enrollState === "done" ? Kirigami.Theme.positiveTextColor
                        : kcm.enrollState === "failed" ? Kirigami.Theme.negativeTextColor
                        : Kirigami.Theme.textColor
                }
            }
            QQC2.Label {
                Layout.fillWidth: true
                visible: kcm.faceHint.length > 0 && kcm.enrollState === "running"
                horizontalAlignment: Text.AlignHCenter
                text: kcm.faceHint
                color: Kirigami.Theme.disabledTextColor
            }
            QQC2.ProgressBar {
                Layout.fillWidth: true
                visible: kcm.enrollState === "running" && kcm.steps > 0
                from: 0
                to: kcm.steps
                value: kcm.progress
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                spacing: Kirigami.Units.smallSpacing

                Item { Layout.fillWidth: true }
                QQC2.Button {
                    visible: !enrollDialog.started
                    icon.name: "media-record"
                    text: i18n("Start Capture")
                    highlighted: true
                    enabled: enrollDialog.refining
                        || (enrollDialog.chosenName.length > 0 && !kcm.hasFace(enrollDialog.chosenName))
                    onClicked: kcm.startEnroll(enrollDialog.chosenName)
                }
                QQC2.Button {
                    visible: kcm.enrollState === "running"
                    icon.name: "dialog-cancel"
                    text: i18n("Cancel")
                    onClicked: enrollDialog.close()
                }
                QQC2.Button {
                    visible: kcm.enrollState === "done" || kcm.enrollState === "failed"
                    icon.name: "dialog-ok"
                    text: i18n("Done")
                    highlighted: true
                    onClicked: enrollDialog.close()
                }
            }
        }
    }

    Kirigami.PromptDialog {
        id: deleteDialog
        property string faceName
        title: i18n("Delete Face")
        subtitle: i18n("Delete “%1”? You can add it again later.", faceName)
        standardButtons: Kirigami.Dialog.Cancel
        customFooterActions: [
            Kirigami.Action {
                text: i18n("Delete")
                icon.name: "edit-delete"
                onTriggered: {
                    kcm.deleteFace(deleteDialog.faceName);
                    deleteDialog.close();
                }
            }
        ]
    }

    Kirigami.PromptDialog {
        id: renameDialog
        property string faceName
        readonly property string newName: nameEdit.text.trim()
        title: i18n("Rename Face")
        standardButtons: Kirigami.Dialog.Cancel | Kirigami.Dialog.Ok
        onAccepted: if (newName.length > 0 && newName !== faceName && !kcm.hasFace(newName)) kcm.renameFace(faceName, newName)
        onOpened: {
            nameEdit.text = faceName;
            nameEdit.forceActiveFocus();
        }
        ColumnLayout {
            spacing: Kirigami.Units.smallSpacing

            QQC2.TextField {
                id: nameEdit
                Layout.fillWidth: true
                onAccepted: renameDialog.accept()
            }
            QQC2.Label {
                Layout.fillWidth: true
                visible: renameDialog.newName !== renameDialog.faceName && kcm.hasFace(renameDialog.newName)
                text: i18n("A face with this name already exists")
                color: Kirigami.Theme.negativeTextColor
                wrapMode: Text.WordWrap
            }
        }
    }
}
