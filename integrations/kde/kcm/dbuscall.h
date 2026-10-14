// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <QDBusMessage>
#include <QLatin1String>
#include <QObject>
#include <QVariantList>
#include <functional>

namespace GazeBus
{
inline constexpr QLatin1String Service("com.gundulabs.Gaze");
inline constexpr QLatin1String Path("/com/gundulabs/Gaze");

using Reply = std::function<void(const QDBusMessage &)>;

// A polkit password prompt can stay open for a while, so calls wait this long for a reply.
inline constexpr int PromptTimeoutMs = 120000;

// Calls a method on the system bus without blocking and allows a polkit prompt.
// The reply reaches done on the owner's thread, and is dropped if the owner is gone.
void call(QObject *owner, const QString &service, const QString &path, const QString &interface, const QString &method, const QVariantList &args, Reply done = {}, int timeoutMs = PromptTimeoutMs);

// The same call on the Gaze daemon.
void callGaze(QObject *owner, const QString &method, const QVariantList &args, Reply done = {});
}
