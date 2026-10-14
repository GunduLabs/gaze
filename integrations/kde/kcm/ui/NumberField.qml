// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import QtQuick
import QtQuick.Controls as QQC2

// A spin box bound to one configuration key, with an optional fractional step.
QQC2.SpinBox {
    id: root

    required property string key
    property real realFrom: 0
    property real realTo: 100
    property real realStep: 1
    property int decimals: 0
    readonly property real factor: Math.pow(10, decimals)

    from: Math.round(realFrom * factor)
    to: Math.round(realTo * factor)
    stepSize: Math.max(1, Math.round(realStep * factor))
    editable: true
    enabled: kcm.settings.editable
    value: Math.round(Number(kcm.settings.values[key] ?? realFrom) * factor)
    validator: DoubleValidator {
        bottom: Math.min(root.realFrom, root.realTo)
        top: Math.max(root.realFrom, root.realTo)
        decimals: root.decimals
        notation: DoubleValidator.StandardNotation
        locale: root.locale.name
    }
    textFromValue: (v, locale) => Number(v / factor).toLocaleString(locale, 'f', decimals)
    valueFromText: (t, locale) => Math.round(Number.fromLocaleString(locale, t) * factor)
    onValueModified: kcm.settings.set(key, value / factor)
}
