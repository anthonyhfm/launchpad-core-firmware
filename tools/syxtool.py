# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2025-2026 Anthony Hofmeister

import sys
import os
import argparse
import struct
import math
from typing import List, Optional
from dataclasses import dataclass

SYSEX_START = 0xF0
SYSEX_END = 0xF7

NOVATION_HEADER = [0x00, 0x20, 0x29, 0x00]
RGB_FIRMWARE_FOOTER = list(b"Firmware")
LEGACY_FIRMWARE_FOOTER = list(b"Firmware")

UPDATE_INIT = 0x71
UPDATE_HEADER = 0x7C
UPDATE_WRITE = 0x72
UPDATE_FINISH = 0x73
UPDATE_FOOTER = 0x76

LPX_FAMILY_ID = 0x02
LPRGB_FAMILY_ID = 0x00

LPX_PRODUCT_ID = 0x0C
LPMINIMK3_PRODUCT_ID = 0x0D
LPPROMK3_PRODUCT_ID = 0x0E
LPSLOTH_PRODUCT_ID = 0x0F
FLPAD_PRODUCT_ID = 0x20
FLPADMINI_PRODUCT_ID = 0x21
LPMK2_PRODUCT_ID = 0x69
LPPRO_PRODUCT_ID = 0x51
LPS_PRODUCT_ID = 0x20
LPMINIMK1_PRODUCT_ID = 0x36

BLOCK_SIZE_BITS = 0x100
BLOCK_SIZE_BYTES = math.ceil(BLOCK_SIZE_BITS / 8)
BLOCK_SIZE_7BITS = math.ceil(BLOCK_SIZE_BITS / 7)

KNOWN_UPDATE_TYPES = {
	UPDATE_INIT,
	UPDATE_HEADER,
	UPDATE_WRITE,
	UPDATE_FINISH,
	UPDATE_FOOTER,
}

@dataclass
class Product:
	flag: str
	name: str
	mode: str
	family_id: Optional[int] = None
	product_id: Optional[int] = None
	legacy_product_id: Optional[int] = None
	block_size_bytes: int = BLOCK_SIZE_BYTES
	# FLPad 437 uses six raw MIDI bytes instead of the older version nibbles.
	version_format: str = "nibbles"

PRODUCTS = [
	Product("/x", "Launchpad X", "lpx", family_id=LPX_FAMILY_ID, product_id=LPX_PRODUCT_ID),
	Product("/minimk3", "Launchpad Mini MK3", "lpx", family_id=LPX_FAMILY_ID, product_id=LPMINIMK3_PRODUCT_ID),
	Product("/lppmk3", "Launchpad Pro MK3", "lpx", family_id=LPX_FAMILY_ID, product_id=LPPROMK3_PRODUCT_ID),
	Product("/mk2", "Launchpad MK2", "rgb", family_id=LPRGB_FAMILY_ID, product_id=LPMK2_PRODUCT_ID),
	Product("/lpp", "Launchpad Pro", "rgb", family_id=LPRGB_FAMILY_ID, product_id=LPPRO_PRODUCT_ID),
	Product("/lps", "Launchpad S", "legacy", legacy_product_id=LPS_PRODUCT_ID),
	Product("/minimk1", "Launchpad Mini MK1", "legacy", legacy_product_id=LPMINIMK1_PRODUCT_ID),
	Product("/sloth", "Sloth Coprocessor", "lpx", family_id=LPX_FAMILY_ID, product_id=LPSLOTH_PRODUCT_ID),
	Product("/flpad", "FLPad", "lpx", family_id=LPX_FAMILY_ID, product_id=FLPAD_PRODUCT_ID, version_format="bytes"),
	Product("/flpadmini", "FLPad Mini", "lpx", family_id=LPX_FAMILY_ID, product_id=FLPADMINI_PRODUCT_ID, version_format="bytes"),
]

class BinToSyx:
	def __init__(self, product: Product, version: str, input_file: str, output_file: Optional[str] = None):
		self.product = product
		self.version = self._parse_version(version)
		self.input_file = input_file
		self.output_file = output_file or self._generate_output_filename(input_file)
		self.input_data = bytearray()
		self.output_data = bytearray()

	def _parse_version(self, version: str) -> List[int]:
		if self.product.version_format == "bytes":
			# Preserve the observed INIT/HEADER bytes without guessing the meaning
			# of each field from one firmware sample. Both 437 files use this value.
			if len(version) != 12:
				raise ValueError("FLPad version must be six raw MIDI bytes (12 hex characters; firmware 437: 010103350000)")
			try:
				parsed = list(bytes.fromhex(version))
			except ValueError:
				raise ValueError("FLPad version must contain hexadecimal byte pairs") from None
			if len(parsed) != 6 or any(v > 0x7F for v in parsed):
				raise ValueError("FLPad version must contain six 7-bit MIDI bytes (00..7F)")
			return parsed
		expected = {3}
		if self.product.mode == "legacy":
			expected.add(6)
		if len(version) not in expected:
			if self.product.mode == "legacy":
				raise ValueError("Legacy version should be 3 or 6 hexadecimal characters long")
			raise ValueError("Version should be 3 hexadecimal characters long")
		parsed = []
		for ch in version:
			if '0' <= ch <= '9':
				parsed.append(ord(ch) - ord('0'))
			else:
				up = ch.upper()
				if 'A' <= up <= 'F':
					parsed.append(ord(up) - ord('A') + 10)
				else:
					raise ValueError(f"Invalid version character: {ch}")
		if self.product.mode == "legacy" and len(parsed) == 3:
			return [0, 0, 0] + parsed
		return parsed

	def _generate_output_filename(self, input_file: str) -> str:
		base, _ = os.path.splitext(input_file)
		return f"{base}.syx"

	def _create_sysex_start(self, msg_type: int) -> bytearray:
		data = bytearray([SYSEX_START])
		data.extend(NOVATION_HEADER)
		data.append(msg_type)
		return data

	def _uint_to_nibbles(self, n: int) -> List[int]:
		out = []
		for _ in range(8):
			out.append((n & 0xF0000000) >> 28)
			n = ((n << 4) & 0xFFFFFFFF)
		return out

	def _crc32(self, data: bytearray) -> int:
		crc = 0xFFFFFFFF
		for b in data:
			crc ^= (b << 24) & 0xFFFFFFFF
			for _ in range(8):
				if crc & 0x80000000:
					crc = ((crc << 1) ^ 0x04C11DB7) & 0xFFFFFFFF
				else:
					crc = (crc << 1) & 0xFFFFFFFF
		return crc

	def _block_size_bytes(self) -> int:
		return self.product.block_size_bytes

	def _block_size_bits(self) -> int:
		return self._block_size_bytes() * 8

	def _block_size_7bits(self) -> int:
		return math.ceil(self._block_size_bits() / 7)

	def _padded_block(self, block_index: int, pad_byte: int) -> bytearray:
		block_size = self._block_size_bytes()
		start = block_index * block_size
		block = bytearray(self.input_data[start:start + block_size])
		if len(block) < block_size:
			block.extend([pad_byte] * (block_size - len(block)))
		return block

	def _legacy_product_bytes(self) -> List[int]:
		if self.product.legacy_product_id is None:
			raise ValueError(f"{self.product.flag} is not a legacy product")
		return [
			(self.product.legacy_product_id >> 8) & 0x7F,
			self.product.legacy_product_id & 0x7F,
		]

	def _legacy_checksum(self, blocks: int) -> int:
		checksum = 0
		for block_index in range(blocks):
			block = self._padded_block(block_index, 0xFF)
			for byte_index, byte in enumerate(block):
				shift = (byte_index % 4) * 8
				checksum = (checksum + ((byte & 0xFF) << shift)) & 0xFFFFFFFF
		return (-checksum) & 0xFFFFFFFF

	def _legacy_source_byte(self, index: int) -> int:
		if index < len(self.input_data):
			return self.input_data[index]
		return 0xFF

	def _legacy_block_payload(self, block_index: int) -> bytearray:
		block_size = self._block_size_bytes()
		payload = bytearray(self._block_size_7bits())
		block_start = block_index * block_size
		write_index = 0

		for source_start in range(block_start, block_start + block_size, 7):
			chunk = [self._legacy_source_byte(source_start + offset) for offset in range(7)]
			packed = [
				(chunk[0] >> 1) & 0x7F,
				((chunk[0] << 6) | (chunk[1] >> 2)) & 0x7F,
				((chunk[1] << 5) | (chunk[2] >> 3)) & 0x7F,
				((chunk[2] << 4) | (chunk[3] >> 4)) & 0x7F,
				((chunk[3] << 3) | (chunk[4] >> 5)) & 0x7F,
				((chunk[4] << 2) | (chunk[5] >> 6)) & 0x7F,
				((chunk[5] << 1) | (chunk[6] >> 7)) & 0x7F,
				chunk[6] & 0x7F,
			]
			remaining = len(payload) - write_index
			if remaining <= 0:
				break
			to_copy = min(remaining, len(packed))
			payload[write_index:write_index + to_copy] = bytes(packed[:to_copy])
			write_index += to_copy

		return payload

	def _create_legacy_init(self) -> bytearray:
		data = self._create_sysex_start(UPDATE_INIT)
		data.extend(self._legacy_product_bytes())
		data.extend(v & 0xF for v in self.version)
		data.append(SYSEX_END)
		return data

	def _write_block(self, block_index: int, update_type: int) -> bytearray:
		data = self._create_sysex_start(update_type)
		if self.product.mode == "legacy":
			data.extend(self._legacy_block_payload(block_index))
			data.append(SYSEX_END)
			return data
		output_bytes = bytearray(self._block_size_7bits())
		is_promk3 = self.product.product_id == LPPROMK3_PRODUCT_ID
		block = self._padded_block(block_index, 0x00 if is_promk3 else 0xFF)
		if is_promk3:
			for k in range(self._block_size_bits()):
				shift = 7 - (k % 8)
				target_index = k // 7
				read_index = k // 8
				bit = (block[read_index] & (1 << shift)) >> shift
				if k % 7 == 0:
					output_bytes[target_index] = 0
				output_bytes[target_index] |= bit << (6 - (k % 7))
		else:
			for k in range(self._block_size_bits()):
				shift = 7 - (k % 8)
				target_index = k // 7
				read_index = k // 8
				bit = (block[read_index] >> shift) & 0x1
				if k % 7 == 0:
					output_bytes[target_index] = 0
				output_bytes[target_index] |= (bit << (6 - (k % 7)))
		data.extend(output_bytes)
		data.append(SYSEX_END)
		return data

	def _convert_modern(self):
		if self.product.family_id is None or self.product.product_id is None:
			raise ValueError(f"{self.product.flag} is not a modern product")
		blocks = max(1, math.ceil(len(self.input_data) / self._block_size_bytes()))
		init_msg = self._create_sysex_start(UPDATE_INIT)
		version_bytes = self.version if self.product.version_format == "bytes" else [0, 0, 0] + self.version
		init_msg.extend([self.product.family_id, self.product.product_id])
		init_msg.extend(version_bytes)
		init_msg.append(SYSEX_END)
		self.output_data.extend(init_msg)
		if self.product.mode == "lpx":
			header = self._create_sysex_start(UPDATE_HEADER)
			header.append(1 if self.product.product_id == LPPROMK3_PRODUCT_ID else 0)
			if self.product.version_format == "bytes":
				header.extend(version_bytes)
			else:
				header.extend(0x30 | v for v in version_bytes)
			header.extend(self._uint_to_nibbles(len(self.input_data)))
			crc = self._crc32(self.input_data)
			if self.product.product_id == LPPROMK3_PRODUCT_ID:
				print(f"CRC32 (SysEx header): 0x{crc:08X}")
			header.extend(self._uint_to_nibbles(crc))
			header.append(SYSEX_END)
			self.output_data.extend(header)
		for j in range(1, blocks):
			self.output_data.extend(self._write_block(j, UPDATE_WRITE))
		self.output_data.extend(self._write_block(0, UPDATE_FINISH))
		if self.product.mode == "rgb":
			footer = self._create_sysex_start(UPDATE_FOOTER)
			footer.append(0x00)
			footer.extend(RGB_FIRMWARE_FOOTER)
			footer.extend([0x00] * 8)
			footer.append(SYSEX_END)
			self.output_data.extend(footer)

	def _convert_legacy(self):
		blocks = max(1, math.ceil(len(self.input_data) / self._block_size_bytes()))
		self.output_data.extend(self._create_legacy_init())
		for j in range(1, blocks):
			self.output_data.extend(self._write_block(j, UPDATE_WRITE))
		self.output_data.extend(self._write_block(0, UPDATE_FINISH))
		footer = self._create_sysex_start(UPDATE_FOOTER)
		footer.append(0x00)
		footer.extend(LEGACY_FIRMWARE_FOOTER)
		footer.extend(self._uint_to_nibbles(self._legacy_checksum(blocks)))
		footer.append(SYSEX_END)
		self.output_data.extend(footer)

	def convert(self):
		with open(self.input_file, "rb") as f:
			self.input_data = bytearray(f.read())
		if (self.product.family_id == LPX_FAMILY_ID and 
			self.product.product_id == LPPROMK3_PRODUCT_ID and 
			len(self.input_data) >= 8):
			internal_crc = self._crc32(self.input_data[:-8])
			self.input_data[-4:] = struct.pack('<I', internal_crc)
			print(f"Pro MK3: Patched internal CRC: 0x{internal_crc:08X}")
		if self.product.mode == "legacy":
			self._convert_legacy()
		else:
			self._convert_modern()
		with open(self.output_file, "wb") as f:
			f.write(self.output_data)
		print(f"Success! Saved to {self.output_file}")

def crc32_bitwise(data: bytes) -> int:
	crc = 0xFFFFFFFF
	for b in data:
		crc ^= (b << 24) & 0xFFFFFFFF
		for _ in range(8):
			if crc & 0x80000000:
				crc = ((crc << 1) ^ 0x04C11DB7) & 0xFFFFFFFF
			else:
				crc = (crc << 1) & 0xFFFFFFFF
	return crc

def uint_from_nibbles(nibs: List[int]) -> int:
	v = 0
	for n in nibs:
		v = ((v << 4) | (n & 0xF)) & 0xFFFFFFFF
	return v

@dataclass
class InitInfo:
	family_id: int
	product_id: int
	version_nibbles: List[int]

@dataclass
class LegacyInitInfo:
	product_id: int
	version_nibbles: List[int]

@dataclass
class LpxHeaderInfo:
	is_promk3: bool
	version_nibbles: List[int]
	total_size: int
	crc32: int

@dataclass
class SysexMsg:
	msg_type: int
	payload: bytes

def parse_sysex_stream(data: bytes) -> List[SysexMsg]:
	msgs: List[SysexMsg] = []
	i = 0
	n = len(data)
	while i < n:
		while i < n and data[i] != SYSEX_START:
			i += 1
		if i >= n:
			break
		start = i
		j = start + 1
		while j < n and data[j] != SYSEX_END:
			j += 1
		if j >= n:
			raise ValueError("Unterminated SysEx message (no 0xF7).")
		segment = data[start+1:j]
		i = j + 1
		if len(segment) < 5:
			raise ValueError("SysEx message too short for Novation header.")
		if list(segment[:4]) != NOVATION_HEADER:
			continue
		msg_type = segment[4]
		payload = segment[5:]
		msgs.append(SysexMsg(msg_type=msg_type, payload=payload))
	if not msgs:
		raise ValueError("No Novation SysEx messages found.")
	return msgs

def decode_block_7bit_to_32(payload37: bytes, out: bytearray, base: int):
	if len(payload37) != BLOCK_SIZE_7BITS:
		raise ValueError(f"Block payload wrong size: {len(payload37)} (expected {BLOCK_SIZE_7BITS})")
	for i in range(BLOCK_SIZE_BYTES):
		out[base + i] = 0
	for k in range(BLOCK_SIZE_BITS):
		src_idx = k // 7
		bit_in_src = 6 - (k % 7)
		bit = (payload37[src_idx] >> bit_in_src) & 0x1
		dst_byte = base + (k // 8)
		dst_bit  = 7 - (k % 8)
		if bit:
			out[dst_byte] |= (1 << dst_bit)

def syxtobin(input_path: str, output_path: Optional[str]):
	with open(input_path, "rb") as f:
		raw = f.read()
	msgs = parse_sysex_stream(raw)
	init: Optional[InitInfo] = None
	legacy_init: Optional[LegacyInitInfo] = None
	lpx_hdr: Optional[LpxHeaderInfo] = None
	write_blocks: List[bytes] = []
	finish_block: Optional[bytes] = None
	family_id: Optional[int] = None
	product_id: Optional[int] = None
	for m in msgs:
		if m.msg_type == UPDATE_INIT:
			if len(m.payload) < 8:
				raise ValueError("INIT payload too short.")
			family_id = m.payload[0]
			product_id = m.payload[1]
			v0, v1, v2 = m.payload[5] & 0xF, m.payload[6] & 0xF, m.payload[7] & 0xF
			init = InitInfo(family_id=family_id, product_id=product_id, version_nibbles=[v0, v1, v2])
		elif m.msg_type not in KNOWN_UPDATE_TYPES and len(m.payload) == 7:
			product_id = ((m.msg_type & 0x7F) << 8) | (m.payload[0] & 0x7F)
			legacy_init = LegacyInitInfo(
				product_id=product_id,
				version_nibbles=[x & 0xF for x in m.payload[1:7]],
			)
		elif m.msg_type == UPDATE_HEADER:
			if len(m.payload) < 1 + 3 + 3 + 8 + 8:
				raise ValueError("HEADER payload too short.")
			is_promk3 = bool(m.payload[0])
			v0 = m.payload[4] & 0xF
			v1 = m.payload[5] & 0xF
			v2 = m.payload[6] & 0xF
			size_nibs = [x & 0xF for x in m.payload[7:15]]
			crc_nibs  = [x & 0xF for x in m.payload[15:23]]
			total_size = uint_from_nibbles(size_nibs)
			crc_val    = uint_from_nibbles(crc_nibs)
			lpx_hdr = LpxHeaderInfo(is_promk3=is_promk3, version_nibbles=[v0, v1, v2], total_size=total_size, crc32=crc_val)
		elif m.msg_type == UPDATE_WRITE:
			if len(m.payload) != BLOCK_SIZE_7BITS:
				raise ValueError(f"WRITE block has size {len(m.payload)} (expected {BLOCK_SIZE_7BITS})")
			write_blocks.append(m.payload)
		elif m.msg_type == UPDATE_FINISH:
			if len(m.payload) != BLOCK_SIZE_7BITS:
				raise ValueError(f"FINISH block has size {len(m.payload)} (expected {BLOCK_SIZE_7BITS})")
			finish_block = m.payload
	if init is None and legacy_init is None:
		raise ValueError("No INIT message found — not a Novation updater .syx?")
	if finish_block is None:
		raise ValueError("No FINISH block found — stream incomplete.")
	if init is not None and (family_id is None or product_id is None):
		raise AssertionError("Parser state inconsistent.")
	num_blocks = len(write_blocks) + 1
	total_bytes_by_blocks = num_blocks * BLOCK_SIZE_BYTES
	if lpx_hdr is not None and init is not None and init.family_id == LPX_FAMILY_ID:
		target_size = lpx_hdr.total_size
	else:
		target_size = total_bytes_by_blocks
	out = bytearray(total_bytes_by_blocks)
	for i, blk in enumerate(write_blocks, start=1):
		decode_block_7bit_to_32(blk, out, i * BLOCK_SIZE_BYTES)
	decode_block_7bit_to_32(finish_block, out, 0)
	out = out[:target_size]
	if output_path is None:
		base, _ = os.path.splitext(input_path)
		output_path = base + ".bin"
	with open(output_path, "wb") as f:
		f.write(out)
	print(f"Success! Saved to {output_path}")

def main():
	p = argparse.ArgumentParser(description="Launchpad firmware SysEx/bin tool", epilog="FLPad / FLPad Mini: version is six raw MIDI bytes as 12 hex characters (firmware 437: 010103350000). Other modern products use three hex version digits.")
	g = p.add_mutually_exclusive_group(required=True)
	g.add_argument("--to-syx", action="store_true")
	g.add_argument("--to-bin", action="store_true")
	known, rest = p.parse_known_args()
	if known.to_syx:
		if len(rest) < 3 or len(rest) > 4:
			print("Usage: syxtool.py --to-syx <product> <version> <input> [output]")
			sys.exit(2)
			
		product_flag = rest[0]
		version = rest[1]
		inp = rest[2]
		outp = rest[3] if len(rest) == 4 else None
		
		try:
			product = next(p for p in PRODUCTS if p.flag == product_flag)
		except StopIteration:
			print(f"Unknown product flag. Known flags: {', '.join(p.flag for p in PRODUCTS)}")
			sys.exit(3)
		try:
			BinToSyx(product, version, inp, outp).convert()
		except Exception as e:
			print(f"Error: {e}")
			sys.exit(1)

	elif known.to_bin:
		if len(rest) < 1 or len(rest) > 2:
			print("Usage: syxtool.py --to-bin <input> [output]")
			sys.exit(2)
		inp = rest[0]
		outp = rest[1] if len(rest) == 2 else None
		try:
			syxtobin(inp, outp)
		except Exception as e:
			print(f"Error: {e}")
			sys.exit(1)

if __name__ == "__main__":
	main()
