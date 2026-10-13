// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    id: page

    readonly property var values: kcm.settings.values
    readonly property bool custom: values["security.level"] === "custom"

    title: i18n("Settings")

    Kirigami.FormLayout {
        Kirigami.InlineMessage {
            Kirigami.FormData.isSection: true
            Layout.fillWidth: true
            visible: kcm.settings.error.length > 0
            type: Kirigami.MessageType.Error
            text: kcm.settings.error
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Security")
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Security level:")
            key: "security.level"
            options: [
                { text: i18n("Low"), value: "low" },
                { text: i18n("Medium"), value: "medium" },
                { text: i18n("High"), value: "high" },
                { text: i18n("Maximum"), value: "maximum" },
                { text: i18n("Custom"), value: "custom" }
            ]
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Detector:")
            key: "security.detector"
            enabled: page.custom
            options: [
                { text: i18n("Standard"), value: "standard" },
                { text: i18n("Accurate"), value: "accurate" }
            ]
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Recognizer:")
            key: "security.recognizer"
            enabled: page.custom
            options: [
                { text: i18n("Standard"), value: "standard" },
                { text: i18n("Accurate"), value: "accurate" }
            ]
        }
        NumberField {
            Kirigami.FormData.label: i18n("Color match threshold:")
            key: "security.rgb_threshold"
            enabled: page.custom
            realFrom: 0.10
            realTo: 0.99
            realStep: 0.01
            decimals: 2
        }
        NumberField {
            Kirigami.FormData.label: i18n("Infrared match threshold:")
            key: "security.ir_threshold"
            enabled: page.custom
            realFrom: 0.10
            realTo: 0.99
            realStep: 0.01
            decimals: 2
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Color and infrared:")
            key: "security.hybrid_policy"
            options: [
                { text: i18n("Default"), value: "default" },
                { text: i18n("Either one matches"), value: "or" },
                { text: i18n("Infrared when dark"), value: "fallback_on_dark" },
                { text: i18n("Both must match"), value: "and" }
            ]
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Hardware")
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Inference:")
            key: "inference.execution_provider"
            options: [
                { text: i18n("CPU"), value: "cpu" },
                { text: i18n("Automatic NPU"), value: "auto" },
                { text: i18n("Intel OpenVINO"), value: "openvino" },
                { text: i18n("AMD Ryzen AI"), value: "vitis" }
            ]
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("OpenVINO device:")
            key: "inference.device"
            enabled: values["inference.execution_provider"] === "openvino"
            options: [
                { text: i18n("CPU"), value: "cpu" },
                { text: i18n("GPU"), value: "gpu" },
                { text: i18n("NPU"), value: "npu" }
            ]
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Color camera:")
            key: "cameras.rgb"
            options: [
                { text: i18n("Not used"), value: "" },
                { text: i18n("Primary camera"), value: "primary" }
            ].concat(kcm.cameraSources())
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Infrared camera:")
            key: "cameras.ir"
            options: [{ text: i18n("Not used"), value: "" }].concat(kcm.cameraSources())
        }
        QQC2.CheckBox {
            id: frameOverride
            Kirigami.FormData.label: i18n("Infrared frame size:")
            text: i18n("Force a size")
            enabled: kcm.settings.loaded
            checked: Number(values["cameras.ir_frame_width"]) > 0
            onToggled: {
                kcm.settings.set("cameras.ir_frame_width", checked ? 640 : 0);
                kcm.settings.set("cameras.ir_frame_height", checked ? 480 : 0);
            }
        }
        RowLayout {
            visible: frameOverride.checked
            spacing: Kirigami.Units.smallSpacing
            NumberField {
                key: "cameras.ir_frame_width"
                realFrom: 1
                realTo: 4096
            }
            QQC2.Label { text: "×" }
            NumberField {
                key: "cameras.ir_frame_height"
                realFrom: 1
                realTo: 4096
            }
        }
        QQC2.Label {
            visible: frameOverride.checked
            text: i18n("Use this if the infrared image is green or corrupted.")
            color: Kirigami.Theme.disabledTextColor
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Infrared emitter:")
            text: i18n("Always turn it on")
            enabled: kcm.settings.loaded
            checked: values["cameras.emitter_enabled"] === true
            onToggled: kcm.settings.set("cameras.emitter_enabled", checked)
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Capture both cameras:")
            key: "cameras.parallel_capture"
            options: [
                { text: i18n("One after the other"), value: "never" },
                { text: i18n("At once when supported"), value: "auto" },
                { text: i18n("Always at once"), value: "always" }
            ]
        }
        NumberField {
            Kirigami.FormData.label: i18n("Darkness cutoff:")
            key: "cameras.dark_luma_threshold"
            realFrom: 0
            realTo: 255
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Enrollment")
        }
        NumberField {
            Kirigami.FormData.label: i18n("Captures per face:")
            key: "enrollment.max_templates"
            realFrom: 1
            realTo: 50
        }
        NumberField {
            Kirigami.FormData.label: i18n("Smallest face size:")
            key: "enrollment.min_face_size_ratio"
            realFrom: 0.10
            realTo: 0.75
            realStep: 0.05
            decimals: 2
        }
        QQC2.Label {
            text: i18n("A lower value accepts a face that is further away.")
            color: Kirigami.Theme.disabledTextColor
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Anti-spoofing")
        }
        QQC2.CheckBox {
            id: liveness
            Kirigami.FormData.label: i18n("Liveness:")
            text: i18n("Reject photos and screens")
            enabled: kcm.settings.loaded
            checked: values["liveness.enabled"] === true
            onToggled: kcm.settings.set("liveness.enabled", checked)
        }
        NumberField {
            Kirigami.FormData.label: i18n("Confidence needed:")
            key: "liveness.threshold"
            enabled: liveness.checked
            realFrom: 0.10
            realTo: 1.0
            realStep: 0.05
            decimals: 2
        }
        NumberField {
            Kirigami.FormData.label: i18n("Longest check (seconds):")
            key: "liveness.max_seconds"
            enabled: liveness.checked
            realFrom: 0.2
            realTo: 30
            realStep: 0.1
            decimals: 1
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Authentication")
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Remote sessions:")
            text: i18n("Do not use face unlock over SSH")
            enabled: kcm.settings.loaded
            checked: values["auth.abort_if_ssh"] === true
            onToggled: kcm.settings.set("auth.abort_if_ssh", checked)
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Laptop lid:")
            text: i18n("Do not use face unlock when it is closed")
            enabled: kcm.settings.loaded
            checked: values["auth.abort_if_lid_closed"] === true
            onToggled: kcm.settings.set("auth.abort_if_lid_closed", checked)
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("After boot:")
            text: i18n("Wait for one suspend and resume first")
            enabled: kcm.settings.loaded
            checked: values["auth.abort_before_first_resume"] === true
            onToggled: kcm.settings.set("auth.abort_before_first_resume", checked)
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Confirmation:")
            text: i18n("Ask before unlocking the screen")
            enabled: kcm.settings.loaded
            checked: values["auth.require_confirmation_lock_screen"] === true
            onToggled: kcm.settings.set("auth.require_confirmation_lock_screen", checked)
        }
        QQC2.CheckBox {
            
            text: i18n("Ask before authorizing administrator actions")
            enabled: kcm.settings.loaded
            checked: values["auth.require_confirmation_elevation"] === true
            onToggled: kcm.settings.set("auth.require_confirmation_elevation", checked)
        }
        NumberField {
            Kirigami.FormData.label: i18n("Wait after waking (ms):")
            key: "auth.resume_grace_ms"
            realFrom: 0
            realTo: 10000
            realStep: 100
        }
        NumberField {
            Kirigami.FormData.label: i18n("Start delay (ms):")
            key: "auth.start_delay_ms"
            realFrom: 0
            realTo: 10000
            realStep: 500
        }
        OptionCombo {
            Kirigami.FormData.label: i18n("Delay applies to:")
            key: "auth.start_delay_scope"
            options: [
                { text: i18n("Every face check, including sudo"), value: "all" },
                { text: i18n("Screen lockers only"), value: "screen_lock" }
            ]
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Storage")
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Face data:")
            text: i18n("Encrypt with a key sealed in the TPM")
            enabled: kcm.settings.loaded
            checked: values["storage.encrypt_templates"] === true
            onToggled: kcm.settings.set("storage.encrypt_templates", checked)
        }
        QQC2.CheckBox {
            Kirigami.FormData.label: i18n("Wallets:")
            text: i18n("Unlock KDE Wallet after a face login")
            enabled: kcm.settings.loaded
            checked: values["storage.unlock_kwallet"] === true
            onToggled: kcm.settings.set("storage.unlock_kwallet", checked)
        }
        QQC2.Label {
            text: i18n("Needs a saved password. Set it up with “gaze keyring --kwallet”.")
            color: Kirigami.Theme.disabledTextColor
        }
        QQC2.CheckBox {
            
            text: i18n("Unlock GNOME Keyring after a face login")
            enabled: kcm.settings.loaded
            checked: values["storage.unlock_gnome_keyring"] === true
            onToggled: kcm.settings.set("storage.unlock_gnome_keyring", checked)
        }
    }
}
