<!--
SPDX-FileCopyrightText: 2026 Gundu Labs
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Inference test fixtures

`identity.onnx` is a 106-byte ONNX graph holding a single `Identity` node over a
1x1 float tensor, at opset 13. It carries no weights and exists only so
`inference::tests` can build a real ONNX Runtime session: session creation is
where ONNX Runtime picks execution providers, and bugs there abort the process
rather than returning an error, so the tests need a model to load rather than a
mock.
