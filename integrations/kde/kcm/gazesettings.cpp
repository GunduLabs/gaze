// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#include "gazesettings.h"

#include <QDBusArgument>
#include <KLocalizedString>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusPendingCallWatcher>
#include <QDBusVariant>

namespace
{
constexpr QLatin1String Service("com.gundulabs.Gaze");
constexpr QLatin1String Path("/com/gundulabs/Gaze");
constexpr int ApplyDelayMs = 400;

struct Field {
    const char *name;
    // D-Bus type: s string, b bool, y byte, u uint32, t uint64, d double
    char type;
};

struct Group {
    const char *name;
    QList<Field> fields;
};

// Mirrors DbusConfig in crates/gaze-core/src/dbus.rs, in wire order.
const QList<Group> &layout()
{
    static const QList<Group> groups = {
        {"inference", {{"execution_provider", 's'}, {"device", 's'}}},
        {"security", {{"level", 's'}, {"detector", 's'}, {"recognizer", 's'}, {"rgb_threshold", 'd'}, {"ir_threshold", 'd'}, {"hybrid_policy", 's'}}},
        {"cameras", {{"rgb", 's'}, {"ir", 's'}, {"emitter_enabled", 'b'}, {"dark_luma_threshold", 'y'}, {"parallel_capture", 's'}}},
        {"auth",
         {{"abort_if_ssh", 'b'},
          {"abort_if_lid_closed", 'b'},
          {"abort_before_first_resume", 'b'},
          {"require_confirmation_lock_screen", 'b'},
          {"require_confirmation_elevation", 'b'},
          {"resume_grace_ms", 't'},
          {"start_delay_ms", 't'},
          {"start_delay_scope", 's'}}},
        {"enrollment", {{"max_templates", 'u'}, {"min_face_size_ratio", 'd'}}},
        {"liveness", {{"enabled", 'b'}, {"threshold", 'd'}, {"max_seconds", 'd'}}},
        {"storage", {{"encrypt_templates", 'b'}}},
    };
    return groups;
}

QVariant readField(const QDBusArgument &in, char type)
{
    switch (type) {
    case 's': {
        QString v;
        in >> v;
        return v;
    }
    case 'b': {
        bool v;
        in >> v;
        return v;
    }
    case 'y': {
        uchar v;
        in >> v;
        return uint(v);
    }
    case 'u': {
        uint v;
        in >> v;
        return v;
    }
    case 't': {
        qulonglong v;
        in >> v;
        return v;
    }
    default: {
        double v;
        in >> v;
        return v;
    }
    }
}

void writeField(QDBusArgument &out, char type, const QVariant &value)
{
    switch (type) {
    case 's':
        out << value.toString();
        break;
    case 'b':
        out << value.toBool();
        break;
    case 'y':
        out << uchar(value.toUInt());
        break;
    case 'u':
        out << value.toUInt();
        break;
    case 't':
        out << value.toULongLong();
        break;
    default:
        out << value.toDouble();
    }
}
}

GazeSettings::GazeSettings(QObject *parent)
    : QObject(parent)
{
    m_applyTimer.setSingleShot(true);
    m_applyTimer.setInterval(ApplyDelayMs);
    connect(&m_applyTimer, &QTimer::timeout, this, &GazeSettings::apply);
    QDBusConnection::systemBus().connect(QStringLiteral("org.freedesktop.systemd1"),
                                         QStringLiteral("/org/freedesktop/systemd1"),
                                         QStringLiteral("org.freedesktop.systemd1.Manager"),
                                         QStringLiteral("JobRemoved"),
                                         this,
                                         SLOT(onJobRemoved(uint, QDBusObjectPath, QString, QString)));
}

void GazeSettings::setError(const QString &error)
{
    if (m_error != error) {
        m_error = error;
        Q_EMIT errorChanged();
    }
}

void GazeSettings::call(const QString &method, const QVariantList &args, Reply done)
{
    auto message = QDBusMessage::createMethodCall(Service, Path, Service, method);
    message.setArguments(args);
    message.setInteractiveAuthorizationAllowed(true);
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [done = std::move(done)](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        done(self->reply());
    });
}

void GazeSettings::load()
{
    auto message = QDBusMessage::createMethodCall(Service, Path, QStringLiteral("org.freedesktop.DBus.Properties"), QStringLiteral("Get"));
    message.setArguments({Service, QStringLiteral("Config")});
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        const QDBusMessage reply = self->reply();
        if (reply.type() == QDBusMessage::ErrorMessage) {
            setError(reply.errorMessage());
            return;
        }
        const QDBusArgument top = reply.arguments().value(0).value<QDBusVariant>().variant().value<QDBusArgument>();
        QVariantMap values;
        top.beginStructure();
        for (const Group &group : layout()) {
            top.beginStructure();
            for (const Field &field : group.fields) {
                values.insert(QLatin1String(group.name) + QLatin1Char('.') + QLatin1String(field.name), readField(top, field.type));
            }
            top.endStructure();
        }
        top.endStructure();
        m_values = values;
        m_loaded = true;
        Q_EMIT editableChanged();
        setError(QString());
        Q_EMIT valuesChanged();

        // These live outside the Config struct.
        call(QStringLiteral("KwalletEnabled"), {}, [this](const QDBusMessage &r) {
            m_values.insert(QStringLiteral("storage.unlock_kwallet"), r.arguments().value(0).toBool());
            Q_EMIT valuesChanged();
        });
        call(QStringLiteral("KeyringEnabled"), {}, [this](const QDBusMessage &r) {
            m_values.insert(QStringLiteral("storage.unlock_gnome_keyring"), r.arguments().value(0).toBool());
            Q_EMIT valuesChanged();
        });
        call(QStringLiteral("IrFrameSize"), {}, [this](const QDBusMessage &r) {
            m_values.insert(QStringLiteral("cameras.ir_frame_width"), r.arguments().value(0).toUInt());
            m_values.insert(QStringLiteral("cameras.ir_frame_height"), r.arguments().value(1).toUInt());
            Q_EMIT valuesChanged();
        });
    });
}

void GazeSettings::unlock()
{
    // Ask polkit directly so the password prompt comes before any edit, like a lock button.
    QDBusArgument subject;
    subject.beginStructure();
    subject << QStringLiteral("system-bus-name");
    subject << QVariantMap{{QStringLiteral("name"), QDBusConnection::systemBus().baseService()}};
    subject.endStructure();

    QDBusArgument details;
    details.beginMap(QMetaType::fromType<QString>(), QMetaType::fromType<QString>());
    details.endMap();

    constexpr uint AllowUserInteraction = 1;
    auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.PolicyKit1"),
                                                  QStringLiteral("/org/freedesktop/PolicyKit1/Authority"),
                                                  QStringLiteral("org.freedesktop.PolicyKit1.Authority"),
                                                  QStringLiteral("CheckAuthorization"));
    message.setArguments({QVariant::fromValue(subject),
                          QStringLiteral("com.gundulabs.gaze.manage-config"),
                          QVariant::fromValue(details),
                          AllowUserInteraction,
                          QString()});
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message, 120000), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        const QDBusMessage reply = self->reply();
        if (reply.type() == QDBusMessage::ErrorMessage) {
            setError(reply.errorMessage());
            return;
        }
        const QDBusArgument result = reply.arguments().value(0).value<QDBusArgument>();
        bool authorized = false;
        result.beginStructure();
        result >> authorized;
        result.endStructure();
        if (authorized != m_unlocked) {
            m_unlocked = authorized;
            Q_EMIT unlockedChanged();
            Q_EMIT editableChanged();
        }
        if (authorized) {
            setError(QString());
        }
    });
}

// An edit still being saved must reach the file first, or the restart kills the daemon mid-save and the edit is lost.
void GazeSettings::restartService()
{
    m_restartWanted = true;
    if (m_applyTimer.isActive()) {
        m_applyTimer.stop();
        apply();
    } else if (!m_applying) {
        sendRestart();
    }
}

void GazeSettings::setRestartState(const QString &state)
{
    if (m_restartState != state) {
        m_restartState = state;
        Q_EMIT restartStateChanged();
    }
}

void GazeSettings::sendRestart()
{
    m_restartWanted = false;
    setRestartState(QStringLiteral("restarting"));
    auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.systemd1"),
                                                  QStringLiteral("/org/freedesktop/systemd1"),
                                                  QStringLiteral("org.freedesktop.systemd1.Manager"),
                                                  QStringLiteral("RestartUnit"));
    message.setArguments({QStringLiteral("gazed.service"), QStringLiteral("replace")});
    message.setInteractiveAuthorizationAllowed(true);
    m_restarting = true;
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message, 120000), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *self) {
        self->deleteLater();
        const QDBusMessage reply = self->reply();
        if (reply.type() == QDBusMessage::ErrorMessage) {
            m_restarting = false;
            setRestartState(QStringLiteral("failed"));
            setError(reply.errorMessage());
            return;
        }
        setError(QString());
    });
}

// systemd answers RestartUnit at once, while the old daemon is still running, so wait for the job to end.
void GazeSettings::onJobRemoved(uint, const QDBusObjectPath &, const QString &unit, const QString &)
{
    if (m_restarting && unit == QLatin1String("gazed.service")) {
        m_restarting = false;
        waitForService(30);
    }
}

// The daemon is not bus activated, so it is unreachable until the restart has registered its name again.
void GazeSettings::waitForService(int attempts)
{
    if (QDBusConnection::systemBus().interface()->isServiceRegistered(QStringLiteral("com.gundulabs.Gaze"))) {
        load();
        setRestartState(QStringLiteral("done"));
        QTimer::singleShot(5000, this, [this] {
            if (m_restartState == QLatin1String("done")) {
                setRestartState(QStringLiteral("idle"));
            }
        });
    } else if (attempts > 0) {
        QTimer::singleShot(500, this, [this, attempts] { waitForService(attempts - 1); });
    } else {
        setRestartState(QStringLiteral("failed"));
        setError(i18n("Gaze did not come back after the restart"));
    }
}

void GazeSettings::set(const QString &key, const QVariant &value)
{
    if (m_values.value(key) == value) {
        return;
    }
    if (key == QLatin1String("security.level") && value.toString() == QLatin1String("custom")) {
        seedCustomSecurity();
    }
    m_values.insert(key, value);
    Q_EMIT valuesChanged();
    m_applyTimer.start();
}

// A preset stores no custom values, and the daemon rejects "custom" without them. Start from what
// the preset resolves to, as the GNOME page does.
void GazeSettings::seedCustomSecurity()
{
    const QString level = m_values.value(QStringLiteral("security.level")).toString();
    if (level == QLatin1String("custom")) {
        return;
    }
    const bool strong = level == QLatin1String("high") || level == QLatin1String("maximum");
    const QString quality = strong ? QStringLiteral("accurate") : QStringLiteral("standard");
    double threshold = 0.4;
    QString policy = QStringLiteral("fallback_on_dark");
    if (level == QLatin1String("low")) {
        threshold = 0.3;
        policy = QStringLiteral("or");
    } else if (level == QLatin1String("high")) {
        threshold = 0.5;
    } else if (level == QLatin1String("maximum")) {
        threshold = 0.6;
        policy = QStringLiteral("and");
    }
    m_values.insert(QStringLiteral("security.detector"), quality);
    m_values.insert(QStringLiteral("security.recognizer"), quality);
    m_values.insert(QStringLiteral("security.rgb_threshold"), threshold);
    m_values.insert(QStringLiteral("security.ir_threshold"), threshold);
    m_values.insert(QStringLiteral("security.hybrid_policy"), policy);
}

void GazeSettings::apply()
{
    QDBusArgument config;
    config.beginStructure();
    for (const Group &group : layout()) {
        config.beginStructure();
        for (const Field &field : group.fields) {
            writeField(config, field.type, m_values.value(QLatin1String(group.name) + QLatin1Char('.') + QLatin1String(field.name)));
        }
        config.endStructure();
    }
    config.endStructure();

    const uint width = m_values.value(QStringLiteral("cameras.ir_frame_width")).toUInt();
    const uint height = m_values.value(QStringLiteral("cameras.ir_frame_height")).toUInt();
    const bool gnome = m_values.value(QStringLiteral("storage.unlock_gnome_keyring")).toBool();
    const bool kwallet = m_values.value(QStringLiteral("storage.unlock_kwallet")).toBool();

    m_applying = true;
    auto finished = [this] {
        m_applying = false;
        if (m_restartWanted) {
            sendRestart();
        }
    };
    auto failed = [this, finished](const QDBusMessage &reply) {
        if (reply.type() == QDBusMessage::ErrorMessage) {
            setError(reply.errorMessage());
            load();
            finished();
            return true;
        }
        return false;
    };
    call(QStringLiteral("SetIrFrameSize"), {width, height}, [this, failed, finished, config, gnome, kwallet](const QDBusMessage &reply) {
        if (failed(reply)) {
            return;
        }
        call(QStringLiteral("SetConfigWithWallets"), {QVariant::fromValue(QDBusVariant(QVariant::fromValue(config))), gnome, kwallet}, [this, failed, finished](const QDBusMessage &r) {
            if (!failed(r)) {
                setError(QString());
                if (!m_restartWanted) {
                    setRestartState(QStringLiteral("suggested"));
                }
                finished();
            }
        });
    });
}
