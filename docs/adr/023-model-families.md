# ADR 023: Model families and a model catalog

Status: accepted (2026-11-07, owner request). The initial family list follows
`research/models.md`.

**Context.**
- The owner wants users to choose between several local models and model architectures for the
  same job, for example BiRefNet or BEN2 for background removal. Laqta implements support for each
  architecture itself.
- Models in the same job differ in input size, normalisation, outputs and post-processing, so a
  plain "pick a file" option would not work.
- ADR 021 left background removal without a port because it had only one adapter.

**Decision.**

1. **One deep module, `laqta-ml`.** It owns everything about local models: the catalog,
   downloads, hash checks, the runtime, sessions, execution providers, and the model families.
   - **Interface:** list the available models per task; prepare a model with download progress;
     run a task (`remove_background`, `ocr`) with a chosen model; validate a custom model.
   - **Inside, private:** the families, the runtime, the session cache, and the release timer.

2. **Model family.** A family is Laqta's implementation of one architecture: how inputs are
   prepared, which outputs are read, and how they become a result.
   - Families are Rust code in `laqta-ml`, one file per family, so each is tested in one place.
   - Families that differ only in numbers (input size, resize mode, colour order, mean and std,
     output index, activation) share one **parametric family**. Their entries carry those
     numbers.
   - Architectures with different steps get their own family, for example multi-output refinement
     or tiling.
   - Each family declares the input and output shapes it accepts, and validation checks a model
     file against them.

3. **Model catalog.** `crates/laqta-ml/catalog/models.toml` ships with each release. Each entry
   holds:
   - id, task, family, and family parameters;
   - display name and description;
   - download URL and SHA-256;
   - size on disk, weights licence, and a commercial-use flag;
   - supported precisions (fp32, fp16, int8).

   The settings UI lists entries per task and shows each one's size, licence and expected speed.
   One entry per task is the default.

4. **Custom models.** In settings, the user can add an ONNX file, from a local path or a URL, as
   a custom entry.
   - The user picks a supported family and fills in its parameters, with the family's defaults
     pre-filled.
   - Laqta validates the input and output shapes, runs a small test image, and only then saves
     the entry.
   - A URL needs a SHA-256 before it is downloaded. A local file is trusted as chosen.
   - Custom entries are stored in settings, in the same format as catalog entries.

5. **Runtime.**
   - Model and onnxruntime downloads go through `laqta-http`'s verified fetch (ADR 024).
   - `ort = { version = "=2.0.0-rc.15", default-features = false, features = ["load-dynamic"] }`, an exact pin (owner decision). Upgrades are deliberate and run the family golden tests. The onnxruntime library is downloaded on first use from a pinned
     build with a pinned hash: the CPU build by default, and the DirectML build on Windows when the
     user picks GPU.
   - The execution provider is a setting: Auto, CPU or GPU. Auto falls back to CPU when a GPU
     provider fails to load or rejects the model.
   - Models that need custom operator libraries are rejected, so no native code is loaded from a
     model.
   - The runtime is an internal seam inside `laqta-ml`. A second backend (`tract`, `candle`) is
     added only if a family needs it.

6. **Memory.** At most one session per task is loaded, and it is released by the lazy-resource
   timer (ARCHITECTURE.md §1.4). Loading a different model releases the previous one first.

7. **Ports.**
   - `Ocr` keeps two production adapters: `laqta-ml` (PP-OCR family) and Windows.Media.Ocr.
   - `BackgroundRemover` returns, because it now has two production adapters, so it meets the
     seam rule (ADR 021):
     - `laqta-ml` (local families);
     - a cloud adapter in `laqta-cloud` (remove.bg, API key in `SecretStore`).

8. **Cloud models already follow this idea.** `ImageAnalyzer` providers are configuration: base
   URL, model name and API key. Any OpenAI-compatible endpoint works, including local Ollama or
   LM Studio.

**Rejected.**
- **Arbitrary model files with no family:** Laqta cannot know how to prepare inputs or read
  outputs, so results would be wrong without any error.
- **User-written pre/post-processing scripts:** this needs a scripting engine, which is a
  non-goal (ARCHITECTURE.md §1.3).
- **One fixed model per task:** it doesn't meet the owner's requirement.
- **A remote catalog fetched at runtime:** a catalog that changes without a release can't be
  reviewed. Catalog updates ship with releases.

**Initial families** (from `research/models.md`, implemented in this order):

| Family | Task | Covers | Notes |
| - | - | - | - |
| A: ImageNet-normalised, fixed square, sigmoid | background removal | BiRefNet (lite, general, matting, HR), BEN2, RMBG-2.0 as a custom model only | Parametric. Default entry: BiRefNet-lite fp16 (115 MB, MIT). BEN2 (219 MB, MIT) takes an `output_activation` parameter until we confirm whether its output is already a sigmoid |
| O1: PP-OCR (DBNet detection + CTC recognition) | OCR | PP-OCRv5 mobile and server detection, per-script recognition models with pinned dictionaries | Apache-2.0. ONNX files from the oar-ocr or RapidOCR mirrors. PP-OCRv6 is added when ONNX exports exist |
| B: rembg U²-Net / IS-Net, min-max output | background removal | u2net, u2netp (5 MB), silueta, IS-Net; RMBG-1.4 as a custom model only | Parametric. IS-Net ships only once its weights licence is settled: the DIS repo says Apache-2.0, the ONNX mirror says AGPL-3.0 |
| C: MODNet | background removal (portraits) | MODNet | Apache-2.0, 13 MB fp16. The output is already alpha |

Not in the first wave: InSPyReNet (no official ONNX), withoutbg (a four-model pipeline), SAM-family
models (need prompts), and transformer OCR (the PP-OCR Latin model covers it).

CPU latency and peak RAM at 1024 px are not published for any of these. A spike in the M5 plan
measures them on the reference machine before defaults are final.

**Consequences.**
- Adding a new architecture is a new family file, a catalog entry, and a golden test: a fixed
  input image and its expected mask or text, compared within a tolerance.
- Weights licences are shown before download. Gated or non-commercial models (for example
  RMBG-2.0) are not in the catalog. Users can still add them as custom models from their own copy.
