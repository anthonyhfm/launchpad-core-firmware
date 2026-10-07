# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Anthony Hofmeister

import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import syxtool


class SyxToolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def encode(self, flag, version, data):
        source = self.root / "input.bin"
        output = self.root / "output.syx"
        source.write_bytes(data)
        product = next(p for p in syxtool.PRODUCTS if p.flag == flag)
        with contextlib.redirect_stdout(io.StringIO()):
            syxtool.BinToSyx(product, version, str(source), str(output)).convert()
        return output

    def decode(self, source):
        output = self.root / "decoded.bin"
        with contextlib.redirect_stdout(io.StringIO()):
            syxtool.syxtobin(str(source), str(output))
        return output.read_bytes()

    def test_flpad_437_headers_and_crc(self):
        # INIT and the six version bytes come directly from the two supplied
        # stock 437 streams. CRC for "123456789" is the CRC-32/MPEG-2 check value.
        for flag, product_id in [("/flpad", 0x20), ("/flpadmini", 0x21)]:
            with self.subTest(flag=flag):
                output = self.encode(flag, "010103350000", b"123456789")
                messages = syxtool.parse_sysex_stream(output.read_bytes())
                self.assertEqual(messages[0].msg_type, 0x71)
                self.assertEqual(messages[0].payload, bytes([2, product_id, 1, 1, 3, 0x35, 0, 0]))
                self.assertEqual(messages[1].msg_type, 0x7C)
                self.assertEqual(messages[1].payload, bytes.fromhex(
                    "00 01 01 03 35 00 00 "
                    "00 00 00 00 00 00 00 09 "
                    "00 03 07 06 0e 06 0e 07"
                ))
                self.assertEqual(self.decode(output), b"123456789")

    def test_block_order_and_ff_padding(self):
        data = bytes(range(65))
        output = self.encode("/flpad", "010103350000", data)
        messages = syxtool.parse_sysex_stream(output.read_bytes())
        self.assertEqual([m.msg_type for m in messages], [0x71, 0x7C, 0x72, 0x72, 0x73])
        # Independent integer unpacking of 37 seven-bit bytes; the final
        # three low bits do not belong to the 256-bit firmware block.
        decoded = []
        for message in messages[2:]:
            packed = 0
            for byte in message.payload:
                packed = (packed << 7) | byte
            decoded.append((packed >> 3).to_bytes(32, "big"))
        self.assertEqual(decoded, [data[32:64], data[64:] + b"\xff" * 31, data[:32]])
        self.assertEqual(self.decode(output), data)

    def test_existing_modern_headers_unchanged(self):
        for flag, product_id in [("/x", 0x0C), ("/minimk3", 0x0D)]:
            with self.subTest(flag=flag):
                output = self.encode(flag, "437", b"123456789")
                messages = syxtool.parse_sysex_stream(output.read_bytes())
                self.assertEqual(messages[0].payload, bytes([2, product_id, 0, 0, 0, 4, 3, 7]))
                self.assertEqual(messages[1].payload[1:7], b"000437")
                self.assertEqual(self.decode(output), b"123456789")

    def test_flpad_rejects_ambiguous_or_invalid_version(self):
        for flag in ["/flpad", "/flpadmini"]:
            for version in ["437", "01010335000G", "010103800000", "01010335000000"]:
                with self.subTest(flag=flag, version=version):
                    with self.assertRaises(ValueError):
                        self.encode(flag, version, b"123456789")





if __name__ == "__main__":
    unittest.main()
