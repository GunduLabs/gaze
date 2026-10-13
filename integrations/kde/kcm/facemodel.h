// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <QAbstractListModel>
#include <QList>

struct Face {
    QString name;
    uint samples;
    bool rgb;
    bool ir;
};

class FaceModel : public QAbstractListModel
{
    Q_OBJECT

public:
    enum Role { NameRole = Qt::UserRole + 1, SamplesRole, RgbRole, IrRole };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    void setFaces(QList<Face> faces);
    bool contains(const QString &name) const;

private:
    QList<Face> m_faces;
};
