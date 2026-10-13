// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <KQuickConfigModule>
#include <QDBusMessage>
#include <QTimer>
#include <functional>

#include "facemodel.h"

class GazeKcm : public KQuickConfigModule
{
    Q_OBJECT
    Q_PROPERTY(FaceModel *faces READ faces CONSTANT)
    Q_PROPERTY(QString user READ user CONSTANT)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged)
    // "idle", "running", "done" or "failed"
    Q_PROPERTY(QString enrollState READ enrollState NOTIFY enrollChanged)
    Q_PROPERTY(QString prompt READ prompt NOTIFY enrollChanged)
    Q_PROPERTY(QString faceHint READ faceHint NOTIFY enrollChanged)
    Q_PROPERTY(uint progress READ progress NOTIFY enrollChanged)
    Q_PROPERTY(uint steps READ steps NOTIFY enrollChanged)
    Q_PROPERTY(QString preview READ preview NOTIFY previewChanged)

public:
    GazeKcm(QObject *parent, const KPluginMetaData &data);

    FaceModel *faces() { return &m_faces; }
    QString user() const { return m_user; }
    QString error() const { return m_error; }
    QString enrollState() const { return m_enrollState; }
    QString prompt() const { return m_prompt; }
    QString faceHint() const { return m_faceHint; }
    uint progress() const { return m_progress; }
    uint steps() const { return m_steps; }
    QString preview() const { return m_preview; }

    Q_INVOKABLE void refresh();
    Q_INVOKABLE void renameFace(const QString &oldName, const QString &newName);
    Q_INVOKABLE void deleteFace(const QString &name);
    Q_INVOKABLE bool hasFace(const QString &name) const;
    Q_INVOKABLE QString suggestedName() const;
    Q_INVOKABLE void startEnroll(const QString &name);
    Q_INVOKABLE void stopEnroll();

Q_SIGNALS:
    void errorChanged();
    void enrollChanged();
    void previewChanged();

private Q_SLOTS:
    void onEnrollStatus(const QDBusMessage &message);
    void onFaceStatus(const QDBusMessage &message);
    void onPreviewFrame(const QDBusMessage &message);

private:
    using Reply = std::function<void(const QDBusMessage &)>;
    void call(const QString &method, const QVariantList &args, Reply done = {});
    void callFaceMethod(const QString &method, const QVariantList &args);
    void finishEnroll(const QString &state, const QString &prompt);
    void release();
    void setError(const QString &error);

    QString m_user;
    QString m_error;
    FaceModel m_faces;
    QString m_enrollState = QStringLiteral("idle");
    QString m_prompt;
    QString m_faceHint;
    uint m_progress = 0;
    uint m_steps = 0;
    QString m_preview;
};
