// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#include "gazekcm.h"

#include <KLocalizedString>
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
constexpr int TestTimeoutMs = 20000;
constexpr int TestResultMs = 4000;

QString enrollPrompt(const QString &id)
{
    if (id == QLatin1String("look-straight")) return i18n("Look straight at the camera");
    if (id == QLatin1String("look-up")) return i18n("Tilt your face slightly up");
    if (id == QLatin1String("look-down")) return i18n("Tilt your face slightly down");
    if (id == QLatin1String("look-left")) return i18n("Turn your face slightly left");
    if (id == QLatin1String("look-right")) return i18n("Turn your face slightly right");
    if (id == QLatin1String("captured")) return i18n("Captured");
    if (id == QLatin1String("completed")) return i18n("Face added");
    if (id == QLatin1String("cancelled")) return i18n("Enrollment cancelled");
    if (id == QLatin1String("db-failed")) return i18n("Could not save the face");
    if (id == QLatin1String("camera-failed")) return i18n("Camera error during enrollment");
    return id;
}

QString hintText(const QString &id)
{
    if (id == QLatin1String("no-face")) return i18n("Look at the camera");
    if (id == QLatin1String("too-dark")) return i18n("More light is needed");
    if (id == QLatin1String("clipped")) return i18n("Move back, your face is cut off");
    if (id == QLatin1String("not-centered")) return i18n("Center your face");
    if (id == QLatin1String("too-far")) return i18n("Come closer");
    if (id == QLatin1String("too-close")) return i18n("Move back a little");
    if (id == QLatin1String("ready") || id == QLatin1String("usable")) return i18n("Hold still");
    return {};
}
}

GazeKcm::GazeKcm(QObject *parent, const KPluginMetaData &data)
    : KQuickConfigModule(parent, data)
{
    const passwd *entry = getpwuid(getuid());
    m_user = entry ? QString::fromLocal8Bit(entry->pw_name) : QString();
    setButtons(NoAdditionalButton);

    auto bus = QDBusConnection::systemBus();
    bus.connect(Service, Path, Service, QStringLiteral("EnrollStatus"), this, SLOT(onEnrollStatus(QDBusMessage)));
    bus.connect(Service, Path, Service, QStringLiteral("FaceStatus"), this, SLOT(onFaceStatus(QDBusMessage)));
    bus.connect(Service, Path, Service, QStringLiteral("PreviewFrame"), this, SLOT(onPreviewFrame(QDBusMessage)));
    bus.connect(Service, Path, Service, QStringLiteral("VerifyStatus"), this, SLOT(onVerifyStatus(QDBusMessage)));

    m_testTimeout.setSingleShot(true);
    m_testTimeout.setInterval(TestTimeoutMs);
    connect(&m_testTimeout, &QTimer::timeout, this, [this] {
        call(QStringLiteral("VerifyStop"), {});
        finishTest(QStringLiteral("timeout"));
    });

    // The verdict is a notice, not state, so it clears itself.
    m_testReset.setSingleShot(true);
    m_testReset.setInterval(TestResultMs);
    connect(&m_testReset, &QTimer::timeout, this, [this] {
        m_testState = QStringLiteral("idle");
        Q_EMIT testChanged();
    });

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

QString GazeKcm::suggestedName() const
{
    const QString base = QStringLiteral("default");
    if (!m_faces.contains(base)) {
        return base;
    }
    for (int n = 2;; ++n) {
        const QString name = QStringLiteral("face-%1").arg(n);
        if (!m_faces.contains(name)) {
            return name;
        }
    }
}

void GazeKcm::release()
{
    call(QStringLiteral("Release"), {});
}

void GazeKcm::finishEnroll(const QString &state, const QString &prompt)
{
    m_enrollState = state;
    m_prompt = prompt;
    m_faceHint.clear();
    Q_EMIT enrollChanged();
    release();
    refresh();
}

void GazeKcm::startEnroll(const QString &name)
{
    if (m_enrollState == QLatin1String("running") || m_testState == QLatin1String("running")) {
        return;
    }
    m_enrollState = QStringLiteral("running");
    m_prompt = i18n("Starting the camera…");
    m_faceHint.clear();
    m_progress = 0;
    m_steps = 0;
    m_preview.clear();
    Q_EMIT enrollChanged();
    Q_EMIT previewChanged();

    call(QStringLiteral("Claim"), {m_user}, [this, name](const QDBusMessage &claim) {
        if (claim.type() == QDBusMessage::ErrorMessage) {
            finishEnroll(QStringLiteral("failed"), claim.errorMessage());
            return;
        }
        call(QStringLiteral("EnrollStart"), {name}, [this](const QDBusMessage &start) {
            if (start.type() == QDBusMessage::ErrorMessage) {
                finishEnroll(QStringLiteral("failed"), start.errorMessage());
            }
        });
    });
}

void GazeKcm::stopEnroll()
{
    if (m_enrollState == QLatin1String("running")) {
        call(QStringLiteral("EnrollStop"), {});
        finishEnroll(QStringLiteral("idle"), QString());
    } else {
        m_enrollState = QStringLiteral("idle");
        Q_EMIT enrollChanged();
    }
    m_preview.clear();
    Q_EMIT previewChanged();
}

void GazeKcm::onEnrollStatus(const QDBusMessage &message)
{
    if (m_enrollState != QLatin1String("running")) {
        return;
    }
    const QVariantList args = message.arguments();
    if (args.size() < 6) {
        return;
    }
    m_progress = args.at(1).toUInt();
    m_steps = args.at(2).toUInt();
    const bool done = args.at(3).toBool();
    const QString id = args.at(4).toString();
    if (id == QLatin1String("db-failed") || id == QLatin1String("camera-failed") || id == QLatin1String("cancelled")) {
        finishEnroll(QStringLiteral("failed"), enrollPrompt(id));
    } else if (done) {
        const bool saved = id == QLatin1String("completed");
        finishEnroll(saved ? QStringLiteral("done") : QStringLiteral("failed"), saved ? enrollPrompt(id) : i18n("Enrollment finished without saving"));
    } else {
        m_prompt = enrollPrompt(id);
        Q_EMIT enrollChanged();
    }
}

void GazeKcm::onFaceStatus(const QDBusMessage &message)
{
    if (m_enrollState != QLatin1String("running") || message.arguments().isEmpty()) {
        return;
    }
    m_faceHint = hintText(message.arguments().at(0).toString());
    Q_EMIT enrollChanged();
}

void GazeKcm::onPreviewFrame(const QDBusMessage &message)
{
    if (m_enrollState != QLatin1String("running") || message.arguments().isEmpty()) {
        return;
    }
    m_preview = QStringLiteral("data:image/jpeg;base64,") + QString::fromLatin1(message.arguments().at(0).toByteArray().toBase64());
    Q_EMIT previewChanged();
}

void GazeKcm::finishTest(const QString &state)
{
    m_testTimeout.stop();
    m_testState = state;
    m_testReset.start();
    Q_EMIT testChanged();
    release();
}

void GazeKcm::testFace()
{
    if (m_enrollState == QLatin1String("running") || m_testState == QLatin1String("running")) {
        return;
    }
    m_testReset.stop();
    m_testState = QStringLiteral("running");
    Q_EMIT testChanged();
    call(QStringLiteral("Claim"), {m_user}, [this](const QDBusMessage &claim) {
        if (claim.type() == QDBusMessage::ErrorMessage) {
            setError(claim.errorMessage());
            finishTest(QStringLiteral("failed"));
            return;
        }
        call(QStringLiteral("VerifyStart"), {QStringLiteral("any")}, [this](const QDBusMessage &start) {
            if (start.type() == QDBusMessage::ErrorMessage) {
                setError(start.errorMessage());
                finishTest(QStringLiteral("failed"));
                return;
            }
            m_testTimeout.start();
        });
    });
}

void GazeKcm::onVerifyStatus(const QDBusMessage &message)
{
    if (m_testState != QLatin1String("running") || message.arguments().isEmpty()) {
        return;
    }
    const bool matched = message.arguments().at(0).toString() == QLatin1String("verify-match");
    finishTest(matched ? QStringLiteral("success") : QStringLiteral("failed"));
}

#include "gazekcm.moc"
