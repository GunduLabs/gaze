// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#include "gazebus.h"

#include <QDBusConnection>
#include <QDBusPendingCallWatcher>

namespace GazeBus
{
void call(QObject *owner, const QString &service, const QString &path, const QString &interface, const QString &method, const QVariantList &args, Reply done, int timeoutMs)
{
    auto message = QDBusMessage::createMethodCall(service, path, interface, method);
    message.setArguments(args);
    message.setInteractiveAuthorizationAllowed(true);
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message, timeoutMs), owner);
    QObject::connect(watcher, &QDBusPendingCallWatcher::finished, owner, [done = std::move(done)](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        if (done) {
            done(self->reply());
        }
    });
}

void callGaze(QObject *owner, const QString &method, const QVariantList &args, Reply done)
{
    call(owner, Service, Path, Service, method, args, std::move(done));
}

void sendGaze(const QString &method)
{
    QDBusConnection::systemBus().send(QDBusMessage::createMethodCall(Service, Path, Service, method));
}
}
