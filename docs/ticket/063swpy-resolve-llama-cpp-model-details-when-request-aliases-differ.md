# Resolve llama.cpp model details when request aliases differ from catalog IDs

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21

The llama.cpp fixture requests `llamacpp/qwen3.5:9b`, while `/v1/models` reports `unsloth/Qwen3.5-9B-GGUF`. `map_model` strips the vendor prefix to `Qwen3.5-9B-GGUF`, so `model_details("qwen3.5:9b")` misses the loaded model and returns empty details. The accepted snapshot loses the `/props` context window of 8192.

The chat cassettes are also inconsistent with model discovery: chat responses identify a 35B model while `/v1/models` identifies a 9B model.

Acceptance criteria:

- Define and implement model identity matching for llama.cpp request aliases and loaded catalog IDs.
- Preserve the served context window when the configured request name differs from the server model ID.
- Avoid ambiguous fallback when a server exposes more than one model.
- Re-record a self-consistent llama.cpp fixture corpus using one loaded model.
- Make the model-details fixture assert the expected ID and context window instead of accepting empty details.
