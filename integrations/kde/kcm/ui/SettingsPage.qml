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
    // Mirrors the GNOME page: dependent rows are hidden, and wallet unlock needs a prerequisite.
    readonly property bool livenessOn: values["liveness.enabled"] === true
    readonly property bool encryptOn: values["storage.encrypt_templates"] === true
    readonly property bool keyringOn: values["storage.unlock_gnome_keyring"] === true
    readonly property bool kwalletOn: values["storage.unlock_kwallet"] === true
    readonly property bool walletOn: keyringOn || kwalletOn
    readonly property bool walletReady: livenessOn && encryptOn

    readonly property var levels: [
        { text: i18n("Standard"), value: "standard" },
        { text: i18n("Accurate"), value: "accurate" }
    ]

    title: i18n("Settings")

    actions: [
        Kirigami.Action {
            text: i18n("Restart Service")
            icon.name: "view-refresh"
            enabled: kcm.settings.restartState !== "restarting"
            onTriggered: kcm.settings.restartService()
        }
    ]

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.settings.loaded && !kcm.settings.unlocked
            type: Kirigami.MessageType.Information
            text: i18n("Settings are locked")
            actions: [
                Kirigami.Action {
                    text: i18n("Unlock…")
                    icon.name: "unlock"
                    onTriggered: kcm.settings.unlock()
                }
            ]
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: ["suggested", "restarting", "done"].includes(kcm.settings.restartState)
            type: kcm.settings.restartState === "done" ? Kirigami.MessageType.Positive : Kirigami.MessageType.Information
            text: kcm.settings.restartState === "suggested" ? i18n("Settings saved. Restart the Gaze service to apply them.")
                : kcm.settings.restartState === "restarting" ? i18n("Restarting the Gaze service…")
                : i18n("Gaze service restarted")
            actions: [
                Kirigami.Action {
                    visible: kcm.settings.restartState === "suggested"
                    text: i18n("Restart Service")
                    icon.name: "view-refresh"
                    onTriggered: kcm.settings.restartService()
                }
            ]
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.settings.restartState === "failed"
            type: Kirigami.MessageType.Error
            text: i18n("Restarting the Gaze service failed")
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: kcm.settings.error.length > 0
            type: Kirigami.MessageType.Error
            text: kcm.settings.error
        }

        SectionHeader { text: i18n("Security") }
        SettingRow {
            title: i18n("Security level")
            hint: i18n("Adjust the balance between speed and security")
            OptionCombo {
                key: "security.level"
                options: [
                    { text: i18n("Low"), value: "low" },
                    { text: i18n("Medium"), value: "medium" },
                    { text: i18n("High"), value: "high" },
                    { text: i18n("Maximum"), value: "maximum" },
                    { text: i18n("Custom"), value: "custom" }
                ]
            }
        }
        SettingRow {
            title: i18n("Detector level")
            visible: page.custom
            OptionCombo {
                key: "security.detector"
                options: page.levels
            }
        }
        SettingRow {
            title: i18n("Recognizer level")
            visible: page.custom
            OptionCombo {
                key: "security.recognizer"
                options: page.levels
            }
        }
        SettingRow {
            title: i18n("RGB recognizer threshold")
            visible: page.custom
            hint: i18n("Minimum RGB similarity for a match")
            NumberField {
                key: "security.rgb_threshold"
                realFrom: 0.1
                realTo: 0.99
                realStep: 0.01
                decimals: 3
            }
        }
        SettingRow {
            title: i18n("IR recognizer threshold")
            visible: page.custom
            hint: i18n("Minimum IR similarity for a match")
            NumberField {
                key: "security.ir_threshold"
                realFrom: 0.1
                realTo: 0.99
                realStep: 0.01
                decimals: 3
            }
        }
        SettingRow {
            title: i18n("Hybrid combining policy")
            visible: page.custom
            hint: i18n("Combining policy when both RGB and IR cameras are active")
            OptionCombo {
                key: "security.hybrid_policy"
                options: [
                    { text: i18n("Default"), value: "default" },
                    { text: i18n("Or"), value: "or" },
                    { text: i18n("Fallback on dark"), value: "fallback_on_dark" },
                    { text: i18n("And"), value: "and" }
                ]
            }
        }

        SectionHeader { text: i18n("Hardware") }
        SettingRow {
            title: i18n("Inference execution provider")
            hint: i18n("CPU, automatic NPU, Intel OpenVINO, or AMD Ryzen AI")
            OptionCombo {
                key: "inference.execution_provider"
                options: [
                    { text: i18n("CPU"), value: "cpu" },
                    { text: i18n("Automatic (NPU)"), value: "auto" },
                    { text: i18n("Intel OpenVINO"), value: "openvino" },
                    { text: i18n("AMD Ryzen AI"), value: "vitis" }
                ]
            }
        }
        SettingRow {
            title: i18n("OpenVINO inference device")
            visible: values["inference.execution_provider"] === "openvino"
            hint: i18n("The Intel device used for all ONNX models")
            OptionCombo {
                key: "inference.device"
                options: [
                    { text: i18n("CPU"), value: "cpu" },
                    { text: i18n("GPU"), value: "gpu" },
                    { text: i18n("NPU"), value: "npu" }
                ]
            }
        }
        SettingRow {
            title: i18n("RGB camera source")
            OptionCombo {
                key: "cameras.rgb"
                options: [{ text: i18n("Primary camera"), value: "primary" }]
                    .concat(kcm.cameraSources())
                    .concat([{ text: i18n("None (IR only)"), value: "" }])
            }
        }
        SettingRow {
            title: i18n("IR camera source")
            OptionCombo {
                key: "cameras.ir"
                options: [{ text: i18n("None"), value: "" }].concat(kcm.cameraSources())
            }
        }
        SettingRow {
            title: i18n("IR frame size override")
            hint: i18n("Force one resolution if the IR feed is green or corrupted on auto")
            QQC2.Label {
                visible: frameOverride.checked
                text: i18n("Width")
            }
            NumberField {
                visible: frameOverride.checked
                key: "cameras.ir_frame_width"
                realFrom: 1
                realTo: 4096
            }
            QQC2.Label {
                visible: frameOverride.checked
                text: i18n("Height")
            }
            NumberField {
                visible: frameOverride.checked
                key: "cameras.ir_frame_height"
                realFrom: 1
                realTo: 4096
            }
            QQC2.Switch {
                id: frameOverride
                enabled: kcm.settings.editable
                checked: Number(values["cameras.ir_frame_width"]) > 0
                onToggled: {
                    kcm.settings.set("cameras.ir_frame_width", checked ? 640 : 0);
                    kcm.settings.set("cameras.ir_frame_height", checked ? 480 : 0);
                }
            }
        }
        SwitchRow {
            title: i18n("Force IR emitter")
            key: "cameras.emitter_enabled"
            hint: i18n("Override emitter control, only if the IR emitter stays off on its own")
        }
        SettingRow {
            title: i18n("Parallel RGB + IR capture")
            hint: i18n("Faster hybrid authentication, but some webcams cannot stream both sensors at once")
            OptionCombo {
                key: "cameras.parallel_capture"
                options: [
                    { text: i18n("Never (capture RGB, then IR)"), value: "never" },
                    { text: i18n("Auto (parallel when the camera allows it)"), value: "auto" },
                    { text: i18n("Always (capture RGB and IR at once)"), value: "always" }
                ]
            }
        }
        SettingRow {
            title: i18n("Darkness cutoff")
            hint: i18n("Reject frames below this mean brightness (0–255)")
            NumberField {
                key: "cameras.dark_luma_threshold"
                realFrom: 0
                realTo: 255
                realStep: 1
                decimals: 0
            }
        }

        SectionHeader { text: i18n("Enrollment") }
        SettingRow {
            title: i18n("Maximum templates")
            hint: i18n("Number of capture sets stored per face")
            NumberField {
                key: "enrollment.max_templates"
                realFrom: 1
                realTo: 50
                realStep: 1
                decimals: 0
            }
        }
        SettingRow {
            title: i18n("Minimum face size ratio")
            hint: i18n("Smallest accepted face during enrollment; lower allows more distance")
            NumberField {
                key: "enrollment.min_face_size_ratio"
                realFrom: 0.1
                realTo: 0.75
                realStep: 0.05
                decimals: 2
            }
        }

        SectionHeader { text: i18n("Liveness Anti-Spoofing") }
        SwitchRow {
            title: i18n("Enable liveness spoof prevention")
            key: "liveness.enabled"
            allowed: !page.walletOn
            hint: i18n("Analyze face depth/reflectance to prevent photo spoofing")
        }
        SettingRow {
            title: i18n("Liveness threshold")
            visible: page.livenessOn
            hint: i18n("Minimum spoof prevention confidence")
            NumberField {
                key: "liveness.threshold"
                realFrom: 0.1
                realTo: 1.0
                realStep: 0.05
                decimals: 2
            }
        }
        SettingRow {
            title: i18n("Maximum liveness duration")
            visible: page.livenessOn
            hint: i18n("Maximum seconds analyzed for liveness verification")
            NumberField {
                key: "liveness.max_seconds"
                realFrom: 0.2
                realTo: 30
                realStep: 0.1
                decimals: 1
            }
        }

        SectionHeader { text: i18n("Authentication") }
        SwitchRow {
            title: i18n("Abort if SSH")
            key: "auth.abort_if_ssh"
            hint: i18n("Prevent authentication over SSH connections")
        }
        SwitchRow {
            title: i18n("Abort if lid closed")
            key: "auth.abort_if_lid_closed"
            hint: i18n("Prevent authentication when the laptop lid is closed")
        }
        SwitchRow {
            title: i18n("Require a suspend first")
            key: "auth.abort_before_first_resume"
            hint: i18n("Prevent authentication until the system has suspended and resumed once")
        }
        SwitchRow {
            title: i18n("Require confirmation on lock screen")
            key: "auth.require_confirmation_lock_screen"
            hint: i18n("Require pressing Enter or clicking OK to authorize after face matches on the lock screen or login screen")
        }
        SwitchRow {
            title: i18n("Require confirmation for elevated authentication")
            key: "auth.require_confirmation_elevation"
            hint: i18n("Require pressing Enter or clicking OK to authorize after face matches for sudo, polkit, and similar prompts")
        }
        SettingRow {
            title: i18n("Resume grace period (ms)")
            hint: i18n("Delay face authentication on wake from suspend")
            NumberField {
                key: "auth.resume_grace_ms"
                realFrom: 0
                realTo: 10000
                realStep: 100
                decimals: 0
            }
        }
        SettingRow {
            title: i18n("Start delay (ms)")
            hint: i18n("Delay before face authentication starts")
            NumberField {
                key: "auth.start_delay_ms"
                realFrom: 0
                realTo: 10000
                realStep: 500
                decimals: 0
            }
        }
        SettingRow {
            title: i18n("Start delay applies to")
            visible: Number(values["auth.start_delay_ms"]) > 0
            hint: i18n("Which prompts wait for the start delay")
            OptionCombo {
                key: "auth.start_delay_scope"
                options: [
                    { text: i18n("All face authentication (including sudo)"), value: "all" },
                    { text: i18n("Screen lockers only"), value: "screen_lock" }
                ]
            }
        }

        SectionHeader { text: i18n("Storage") }
        SwitchRow {
            title: i18n("Encrypt face templates")
            key: "storage.encrypt_templates"
            allowed: !page.walletOn
            hint: i18n("Encrypt face templates at rest with a TPM-sealed key")
        }
        SwitchRow {
            title: i18n("Unlock GNOME Keyring")
            key: "storage.unlock_gnome_keyring"
            allowed: (page.keyringOn || page.walletReady)
            hint: i18n("Use an enrolled password after liveness-protected GDM or greetd face login")
        }
        SwitchRow {
            title: i18n("Unlock KWallet")
            key: "storage.unlock_kwallet"
            allowed: (page.kwalletOn || page.walletReady)
            hint: i18n("Use an enrolled password after liveness-protected KDE login; enroll with gaze keyring --kwallet")
        }
    }
}
