# testdata

- `contract/` with `fake_tiiny.py`: the contract tests (both servers against a fake Tiiny). CI runs them; no device.
- `*_test.py`, `usb_features.py`: live checks against a running studio and a real Tiiny. Each takes the studio URL,
  labels what it makes `… TEST …` (or saves nothing) and deletes it afterwards. None prints your own gallery, chats,
  voices or vault; `continue_test.py` and `features2_test.py` look up their own test entries in the gallery list.
- `fake_vs_real.py`: compares the fake's answers with the real device's, shapes only (read-only, no values printed).
- `model_store_test.py`: the Model Store end to end through a running studio and a real Tiiny (download, pause, resume,
  cancel, delete and the refusals), with one small model you name that isn't installed; it always removes it again.
- `model_store_ui_check.py`: the Model Store clicked through in a browser against the fake Tiiny (CI runs it).
- `download_probe.py`: what the Tiiny's own download calls do (pause, resume, cancel), straight to the device; `--read-only`
  for the list shapes only.
- `make_docs.py`, `md_dump.py`, `extract_parity.py`: test documents for Text mode and the Python/Rust reader parity check.

Inputs:

- `ref-voice.wav`: a synthetic voice, not a person's: Windows' built-in speech synthesizer (System.Speech, 24 kHz
  mono) reading "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."
- `ocr-test.png`: the text `ocr_test.py` expects. `mug.png`: a 3D input.
