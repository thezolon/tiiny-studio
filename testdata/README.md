# testdata

- `contract/` with `fake_tiiny.py`: the contract tests (both servers against a fake Tiiny). CI runs them; no device.
- `*_test.py`, `usb_features.py`: live checks against a running studio and a real Tiiny. Each takes the studio URL,
  labels what it makes `… TEST …` (or saves nothing) and deletes it afterwards. None prints your own gallery, chats,
  voices or vault; `continue_test.py` and `features2_test.py` look up their own test entries in the gallery list.
- `fake_vs_real.py`: compares the fake's answers with the real device's, shapes only (read-only, no values printed).
- `make_docs.py`, `md_dump.py`, `extract_parity.py`: test documents for Text mode and the Python/Rust reader parity check.

Inputs:

- `ref-voice.wav`: a synthetic voice, not a person's: Windows' built-in speech synthesizer (System.Speech, 24 kHz
  mono) reading "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."
- `ocr-test.png`: the text `ocr_test.py` expects. `mug.png`: a 3D input.
