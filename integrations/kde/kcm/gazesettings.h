// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <QDBusMessage>
#include <QObject>
#include <QTimer>
#include <QVariantMap>
#include <functional>

/**
 * The daemon configuration as a flat key/value map for QML.
 *
 * The Config property is a nested D-Bus struct with a fixed wire layout, so each group
 * and field is listed once in the .cpp and read and written by that table.
 */
class GazeSettings : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QVariantMap values READ values NOTIFY valuesChanged)
    Q_PROPERTY(bool loaded READ loaded NOTIFY valuesChanged)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged)

public:
    explicit GazeSettings(QObject *parent = nullptr);

    QVariantMap values() const { return m_values; }
    bool loaded() const { return m_loaded; }
    QString error() const { return m_error; }

    Q_INVOKABLE void load();
    // Keys look like "security.level" or "storage.unlock_kwallet". Writes are batched.
    Q_INVOKABLE void set(const QString &key, const QVariant &value);

Q_SIGNALS:
    void valuesChanged();
    void errorChanged();

private:
    using Reply = std::function<void(const QDBusMessage &)>;
    void call(const QString &method, const QVariantList &args, Reply done);
    void setError(const QString &error);
    void apply();

    QVariantMap m_values;
    bool m_loaded = false;
    QString m_error;
    QTimer m_applyTimer;
};
