#!/usr/bin/env bash
set -euo pipefail

# Downloads NVIDIA Parakeet TDT model files from HuggingFace
# Model: istupakov/parakeet-tdt-0.6b-v3-onnx (ONNX version of nvidia/parakeet-tdt-0.6b-v3)

REPO="istupakov/parakeet-tdt-0.6b-v3-onnx"
BASE_URL="https://huggingface.co/${REPO}/resolve/main"

DATA_ROOT="${XDG_DATA_HOME:-"$HOME/.local/share"}/hyprwhspr-rs"
MODEL_DIR="${DATA_ROOT}/models/parakeet/parakeet-tdt-0.6b-v3-onnx"

mkdir -p "${MODEL_DIR}"

files=(
    "encoder-model.onnx"
    "encoder-model.onnx.data"
    "decoder_joint-model.onnx"
    "vocab.txt"
)

echo "Downloading Parakeet TDT model files into ${MODEL_DIR}"
echo ""

for f in "${files[@]}"; do
    dest="${MODEL_DIR}/${f}"
    if [ -f "${dest}" ]; then
        echo "  [skip] ${f} (already exists)"
        continue
    fi
    echo "  [download] ${f}..."
    curl -L --progress-bar "${BASE_URL}/${f}" -o "${dest}"
done

echo ""
echo "Parakeet TDT model ready in ${MODEL_DIR}"
echo ""
echo "To use Parakeet TDT, set in your config:"
echo '  "transcription": {'
echo '    "provider": "parakeet"'
echo '  }'
