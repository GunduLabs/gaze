// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#include "gazekcm.h"

#include <KPluginFactory>
#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusPendingCallWatcher>

#include <pwd.h>
#include <unistd.h>

K_PLUGIN_CLASS_WITH_JSON(GazeKcm, "kcm_gaze.json")

namespace
{
constexpr QLatin1String Service("com.gundulabs.Gaze");
constexpr QLatin1String Path("/com/gundulabs/Gaze");
}

GazeKcm::GazeKcm(QObject *parent, const KPluginMetaData &data)
    : KQuickConfigModule(parent, data)
{
    const passwd *entry = getpwuid(getuid());
    m_user = entry ? QString::fromLocal8Bit(entry->pw_name) : QString();
    setButtons(NoAdditionalButton);
    refresh();
}

void GazeKcm::setError(const QString &error)
{
    if (m_error != error) {
        m_error = error;
        Q_EMIT errorChanged();
    }
}

void GazeKcm::call(const QString &method, const QVariantList &args, Reply done)
{
    auto message = QDBusMessage::createMethodCall(Service, Path, Service, method);
    message.setArguments(args);
    message.setInteractiveAuthorizationAllowed(true);
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [done = std::move(done)](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        if (done) {
            done(self->reply());
        }
    });
}

void GazeKcm::refresh()
{
    call(QStringLiteral("ListFaces"), {m_user}, [this](const QDBusMessage &reply) {
        if (reply.type() == QDBusMessage::ErrorMessage) {
            // A user without a profile yet is an empty list, not a failure.
            const bool noProfile = reply.errorMessage().contains(QLatin1String("not found"), Qt::CaseInsensitive);
            m_faces.setFaces({});
            setError(noProfile ? QString() : reply.errorMessage());
            return;
        }
        QList<Face> faces;
        const QDBusArgument list = reply.arguments().value(0).value<QDBusArgument>();
        list.beginArray();
        while (!list.atEnd()) {
            Face face;
            list.beginStructure();
            list >> face.name >> face.samples >> face.rgb >> face.ir;
            list.endStructure();
            faces.append(face);
        }
        list.endArray();
        m_faces.setFaces(std::move(faces));
        setError(QString());
    });
}

void GazeKcm::callFaceMethod(const QString &method, const QVariantList &args)
{
    call(method, args, [this](const QDBusMessage &reply) {
        setError(reply.type() == QDBusMessage::ErrorMessage ? reply.errorMessage() : QString());
        refresh();
    });
}

void GazeKcm::renameFace(const QString &oldName, const QString &newName)
{
    callFaceMethod(QStringLiteral("RenameFace"), {m_user, oldName, newName});
}

void GazeKcm::deleteFace(const QString &name)
{
    callFaceMethod(QStringLiteral("DeleteFace"), {m_user, name});
}

bool GazeKcm::hasFace(const QString &name) const
{
    return m_faces.contains(name);
}

#include "gazekcm.moc"
