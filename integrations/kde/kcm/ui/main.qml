// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.ScrollViewKCM {
    id: root

    title: i18n("Enrolled Faces")
    implicitWidth: Kirigami.Units.gridUnit * 36
    implicitHeight: Kirigami.Units.gridUnit * 30

    header: ColumnLayout {
        spacing: Kirigami.Units.smallSpacing

        QQC2.Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: i18n("Registered face profiles for %1", kcm.user)
            color: Kirigami.Theme.disabledTextColor
            wrapMode: Text.WordWrap
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.error.length > 0
            type: Kirigami.MessageType.Error
            text: kcm.error
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
            text: i18n("No faces added")
            explanation: i18n("Add your face to unlock the screen and authenticate by looking at the camera.")
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
                        { label: i18n("RGB"), on: rgb },
                        { label: i18n("IR"), on: ir }
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
                    onClicked: {
                        renameDialog.faceName = row.name;
                        renameDialog.open();
                    }
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                }
                QQC2.ToolButton {
                    icon.name: "edit-delete"
                    text: i18n("Delete")
                    display: QQC2.AbstractButton.IconOnly
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
        title: i18n("Rename Face")
        standardButtons: Kirigami.Dialog.Cancel | Kirigami.Dialog.Ok
        onAccepted: if (nameEdit.text.length > 0 && nameEdit.text !== faceName && !kcm.hasFace(nameEdit.text)) kcm.renameFace(faceName, nameEdit.text)
        onOpened: {
            nameEdit.text = faceName;
            nameEdit.forceActiveFocus();
        }
        QQC2.TextField {
            id: nameEdit
            onAccepted: renameDialog.accept()
        }
    }
}
