#!/usr/bin/env python3
"""Read-only Windows icon acceptance check; no EXE code is loaded or launched.

Example: python verify-icon-resources.py --binary ARTEX.exe --source-ico
assets/branding/artex-app-icon.ico --window-pid 1234 --output icon-review.json
Window HICONs are borrowed. ExtractIconEx HICONs and GetIconInfo bitmaps are owned.
"""

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import time
import zlib


SIZES = (16, 24, 32, 48, 64, 128, 256)
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


class VerificationError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise VerificationError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    with path.open("rb") as stream:
        result = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
        return result.hexdigest()


def png_shape(data, size):
    # No PNG decoder or image edits: the exact resource bytes are the authority.
    require(data[:8] == PNG_MAGIC, f"Frame {size}: expected PNG payload")
    require(len(data) >= 33 and data[8:16] == b"\0\0\0\rIHDR", f"Frame {size}: missing IHDR")
    width, height, bits, color, compression, filtering, interlace = struct.unpack_from(
        ">IIBBBBB", data, 16
    )
    require((width, height) == (size, size), f"Frame {size}: PNG dimension mismatch")
    require((bits, color, compression, filtering, interlace) == (8, 6, 0, 0, 0),
            f"Frame {size}: expected noninterlaced 8-bit RGBA (32-bit) PNG")
    require(zlib.crc32(data[12:29]) == struct.unpack_from(">I", data, 29)[0],
            f"Frame {size}: invalid IHDR CRC")


def source_frames(path):
    require(path.stat().st_size <= 4 * 1024 * 1024, "Source ICO exceeds 4 MiB")
    data = path.read_bytes()
    require(len(data) >= 6, "Source ICO header truncated")
    require(struct.unpack_from("<HHH", data) == (0, 1, 7), "Source ICO must contain seven icons")
    require(len(data) >= 118, "Source ICO directory truncated")
    frames, ranges = {}, []
    for index, size in enumerate(SIZES):
        w, h, colors, reserved, planes, bpp, length, offset = struct.unpack_from(
            "<BBBBHHII", data, 6 + 16 * index
        )
        require((w or 256, h or 256, colors, reserved, planes, bpp) == (size, size, 0, 0, 1, 32),
                f"Source ICO frame {index}: expected {size}x{size}, planes=1, RGBA32")
        require(length > 0 and offset >= 118 and offset + length <= len(data),
                f"Source ICO frame {size}: payload outside file")
        require(all(offset + length <= start or offset >= end for start, end in ranges),
                f"Source ICO frame {size}: overlapping payload")
        ranges.append((offset, offset + length))
        payload = data[offset:offset + length]
        png_shape(payload, size)
        frames[size] = payload
    return frames, digest(data)


class ICONINFO(ctypes.Structure):
    _fields_ = [("fIcon", wintypes.BOOL), ("xHotspot", wintypes.DWORD),
                ("yHotspot", wintypes.DWORD), ("hbmMask", ctypes.c_void_p),
                ("hbmColor", ctypes.c_void_p)]


class BITMAP(ctypes.Structure):
    _fields_ = [("bmType", ctypes.c_int32), ("bmWidth", ctypes.c_int32),
                ("bmHeight", ctypes.c_int32), ("bmWidthBytes", ctypes.c_int32),
                ("bmPlanes", ctypes.c_uint16), ("bmBitsPixel", ctypes.c_uint16),
                ("bmBits", ctypes.c_void_p)]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int32),
                ("biHeight", ctypes.c_int32), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biXPelsPerMeter", ctypes.c_int32),
                ("biYPelsPerMeter", ctypes.c_int32), ("biClrUsed", ctypes.c_uint32),
                ("biClrImportant", ctypes.c_uint32)]


class WinAPI:
    def __init__(self):
        require(os.name == "nt", "Icon resources and HICON inspection require Windows")
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.user = ctypes.WinDLL("user32", use_last_error=True)
        self.gdi = ctypes.WinDLL("gdi32", use_last_error=True)
        self.shell = ctypes.WinDLL("shell32", use_last_error=True)
        ptr, uint, boolean = ctypes.c_void_p, ctypes.c_uint32, wintypes.BOOL
        self.bind(self.kernel, "LoadLibraryExW", [wintypes.LPCWSTR, ptr, uint], ptr)
        self.bind(self.kernel, "FreeLibrary", [ptr], boolean)
        self.bind(self.kernel, "FindResourceW", [ptr, ptr, ptr], ptr)
        self.bind(self.kernel, "LoadResource", [ptr, ptr], ptr)
        self.bind(self.kernel, "SizeofResource", [ptr, ptr], uint)
        self.bind(self.kernel, "LockResource", [ptr], ptr)
        self.bind(self.kernel, "OpenProcess", [uint, boolean, uint], ptr)
        self.bind(self.kernel, "CloseHandle", [ptr], boolean)
        self.bind(self.kernel, "QueryFullProcessImageNameW",
                  [ptr, uint, wintypes.LPWSTR, ctypes.POINTER(uint)], boolean)
        self.bind(self.shell, "ExtractIconExW",
                  [wintypes.LPCWSTR, ctypes.c_int32, ctypes.POINTER(ptr), ctypes.POINTER(ptr), uint], uint)
        self.bind(self.user, "DestroyIcon", [ptr], boolean)
        self.bind(self.user, "GetIconInfo", [ptr, ctypes.POINTER(ICONINFO)], boolean)
        self.bind(self.user, "GetDC", [ptr], ptr)
        self.bind(self.user, "ReleaseDC", [ptr, ptr], ctypes.c_int32)
        self.bind(self.gdi, "GetObjectW", [ptr, ctypes.c_int32, ptr], ctypes.c_int32)
        self.bind(self.gdi, "DeleteObject", [ptr], boolean)
        self.bind(self.gdi, "GetDIBits", [ptr, ptr, uint, uint, ptr, ptr, uint], ctypes.c_int32)
        self.bind(self.user, "GetWindowThreadProcessId", [ptr, ctypes.POINTER(uint)], uint)
        self.bind(self.user, "IsWindowVisible", [ptr], boolean)
        self.bind(self.user, "GetWindow", [ptr, uint], ptr)
        self.bind(self.user, "SendMessageTimeoutW",
                  [ptr, uint, ctypes.c_size_t, ctypes.c_ssize_t, uint, uint,
                   ctypes.POINTER(ctypes.c_size_t)], ctypes.c_ssize_t)
        self.class_icon = getattr(self.user, "GetClassLongPtrW", None)
        if self.class_icon is None:  # The 32-bit API uses GetClassLongW.
            self.class_icon = self.user.GetClassLongW
        self.class_icon.argtypes = [ptr, ctypes.c_int32]
        self.class_icon.restype = ctypes.c_size_t
        self.enum_callback = ctypes.WINFUNCTYPE(boolean, ptr, ctypes.c_ssize_t)
        self.bind(self.user, "EnumWindows", [self.enum_callback, ctypes.c_ssize_t], boolean)

    @staticmethod
    def bind(dll, name, args, result):
        call = getattr(dll, name)
        call.argtypes = args
        call.restype = result

    @staticmethod
    def checked(value, operation):
        if not value:
            code = ctypes.get_last_error()
            raise VerificationError(f"{operation} failed: Win32 error {code} ({ctypes.FormatError(code).strip()})")
        return value

    def resource(self, module, kind, identifier):
        resource = self.checked(self.kernel.FindResourceW(module, ctypes.c_void_p(identifier),
                                                         ctypes.c_void_p(kind)),
                                f"FindResourceW(type={kind},id={identifier})")
        length = self.checked(self.kernel.SizeofResource(module, resource), "SizeofResource")
        require(length <= 4 * 1024 * 1024, "Icon resource exceeds 4 MiB")
        loaded = self.checked(self.kernel.LoadResource(module, resource), "LoadResource")
        pointer = self.checked(self.kernel.LockResource(loaded), "LockResource")
        # Resource storage is borrowed from the module and released by FreeLibrary.
        return ctypes.string_at(pointer, length)

    def bitmap_pixels(self, bitmap, width, height):
        header = BITMAPINFOHEADER()
        header.biSize = ctypes.sizeof(header)
        header.biWidth, header.biHeight = width, -height  # top-down BGRA
        header.biPlanes, header.biBitCount = 1, 32
        header.biSizeImage = width * height * 4
        pixels = ctypes.create_string_buffer(header.biSizeImage)
        dc = self.checked(self.user.GetDC(None), "GetDC")
        try:
            rows = self.gdi.GetDIBits(dc, bitmap, 0, height, pixels, ctypes.byref(header), 0)
            require(rows == height, f"GetDIBits returned {rows}/{height} rows")
            return pixels.raw
        finally:
            self.user.ReleaseDC(None, dc)

    def inspect_icon(self, icon, label, expected_size=None):
        require(bool(icon), f"{label}: no HICON")
        info = ICONINFO()
        try:
            self.checked(self.user.GetIconInfo(icon, ctypes.byref(info)), f"{label}: GetIconInfo")
            require(info.fIcon and info.hbmColor, f"{label}: expected a color icon")
            bitmap = BITMAP()
            self.checked(self.gdi.GetObjectW(info.hbmColor, ctypes.sizeof(bitmap), ctypes.byref(bitmap)),
                         f"{label}: GetObjectW")
            width, height = bitmap.bmWidth, abs(bitmap.bmHeight)
            require(16 <= width <= 256 and 16 <= height <= 256, f"{label}: expected 16..256px icon dimensions")
            require(width == height, f"{label}: nonsquare icon")
            if expected_size is not None:
                require(width == expected_size, f"{label}: expected {expected_size}px, got {width}px")
            require(bitmap.bmPlanes == 1 and bitmap.bmBitsPixel == 32, f"{label}: expected RGBA32 bitmap")
            pixels = self.bitmap_pixels(info.hbmColor, width, height)
            opaque, blue, samples = 0, 0, []
            for offset in range(0, len(pixels), 4):
                b, g, r, a = pixels[offset:offset + 4]
                if a >= 240:
                    opaque += 1
                    # Approved artwork has an opaque light-blue A. Default green
                    # icons cannot satisfy this blue-dominant sample check.
                    if b >= 120 and b - r >= 25 and b - g >= 5 and g - r >= 15:
                        blue += 1
                        if len(samples) < 8:
                            index = offset // 4
                            samples.append({"x": index % width, "y": index // width,
                                            "rgba": [r, g, b, a]})
            count = width * height
            require(opaque >= count * 0.40, f"{label}: opaque artwork ratio too low ({opaque}/{count})")
            # All seven approved frames have 13.4%-15.6% opaque blue pixels.
            # A few incidental blue pixels in a system/default icon are not proof.
            require(blue >= count * 0.10, f"{label}: approved blue artwork missing ({blue}/{count})")
            return {"width": width, "height": height, "bits_per_pixel": bitmap.bmBitsPixel,
                    "opaque_ratio": round(opaque / count, 6), "blue_opaque_ratio": round(blue / count, 6),
                    "blue_opaque_samples": samples}
        finally:
            if info.hbmColor:
                self.gdi.DeleteObject(info.hbmColor)
            if info.hbmMask:
                self.gdi.DeleteObject(info.hbmMask)

    def verify_resources(self, binary, frames):
        module = self.checked(self.kernel.LoadLibraryExW(str(binary), None, 0x02 | 0x20), "LoadLibraryExW")
        try:
            group = self.resource(module, 14, 1)  # RT_GROUP_ICON, exact ID 1
            require(len(group) == 6 + 14 * 7, "RT_GROUP_ICON ID 1 must have seven exact directory entries")
            require(struct.unpack_from("<HHH", group) == (0, 1, 7), "Invalid RT_GROUP_ICON ID 1 header")
            verified, seen = [], set()
            for index, size in enumerate(SIZES):
                w, h, colors, reserved, planes, bpp, length, resource_id = struct.unpack_from(
                    "<BBBBHHIH", group, 6 + 14 * index
                )
                require((w or 256, h or 256, colors, reserved, planes, bpp) == (size, size, 0, 0, 1, 32),
                        f"PE frame {size}: directory metadata mismatch")
                require(resource_id > 0 and resource_id not in seen, "Duplicate or zero RT_ICON ID")
                seen.add(resource_id)
                payload = self.resource(module, 3, resource_id)
                require(len(payload) == length, f"PE frame {size}: group length differs from RT_ICON")
                require(payload == frames[size], f"PE frame {size}: resource differs from approved ICO bytes")
                verified.append({"size": size, "resource_id": resource_id, "length": length,
                                 "sha256": digest(payload)})
            return {"group_resource_id": 1, "frames": verified, "exact_source_match": True}
        finally:
            self.kernel.FreeLibrary(module)

    def verify_shell(self, binary):
        large, small = ctypes.c_void_p(), ctypes.c_void_p()
        try:
            count = self.shell.ExtractIconExW(str(binary), 0, ctypes.byref(large), ctypes.byref(small), 1)
            require(count == 1 and large.value and small.value, "ExtractIconExW did not extract both large/small icons")
            return {"large": self.inspect_icon(large, "Shell large"),
                    "small": self.inspect_icon(small, "Shell small")}
        finally:
            # ExtractIconEx transfers ownership, unlike WM_GETICON/class handles.
            for handle in {large.value, small.value} - {None}:
                self.user.DestroyIcon(handle)

    def verify_windows(self, pid, binary):
        process = self.checked(self.kernel.OpenProcess(0x1000, False, pid), "OpenProcess(query only)")
        try:
            buffer = ctypes.create_unicode_buffer(32768)
            length = ctypes.c_uint32(len(buffer))
            self.checked(self.kernel.QueryFullProcessImageNameW(process, 0, buffer, ctypes.byref(length)),
                         "QueryFullProcessImageNameW")
            require(os.path.samefile(buffer.value, binary), "Window PID does not belong to the verified binary")
        finally:
            self.kernel.CloseHandle(process)
        windows = []

        def collect(hwnd, _):
            owner = ctypes.c_uint32()
            self.user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
            if owner.value == pid and self.user.IsWindowVisible(hwnd) and not self.user.GetWindow(hwnd, 4):
                windows.append(hwnd)
            return True

        callback = self.enum_callback(collect)  # keep alive across EnumWindows
        deadline = time.monotonic() + 15.0
        while True:
            windows.clear()
            self.checked(self.user.EnumWindows(callback, 0), "EnumWindows")
            if windows:
                break
            remaining = deadline - time.monotonic()
            require(remaining > 0,
                    f"Window appearance deadline: PID {pid} had no visible unowned top-level window within 15 seconds")
            time.sleep(min(0.05, remaining))
        result = []
        for hwnd in windows:
            icons = {}
            for label, kind, class_index in [("large", 1, -14), ("small", 0, -34)]:
                icon = ctypes.c_size_t()
                self.checked(self.user.SendMessageTimeoutW(hwnd, 0x007F, kind, 0, 0x0002, 2000,
                                                          ctypes.byref(icon)), f"WM_GETICON {label}")
                source = "WM_GETICON"
                if not icon.value and label == "small":
                    self.checked(self.user.SendMessageTimeoutW(hwnd, 0x007F, 2, 0, 0x0002, 2000,
                                                              ctypes.byref(icon)), "WM_GETICON SMALL2")
                    source = "WM_GETICON SMALL2"
                if not icon.value:
                    icon.value = self.class_icon(hwnd, class_index)
                    source = "class fallback"
                icons[label] = self.inspect_icon(icon.value, f"Window {label}")
                icons[label]["source"] = source
            result.append({"hwnd": hex(hwnd), "icons": icons})
        return {"pid": pid, "windows": result, "read_only_messages": ["WM_GETICON"],
                "native_supplied_png_size": {"width": 256, "height": 256},
                "actual_dimensions_basis": "Observed HICON bitmaps: Windows/eframe scales the supplied PNG to OS/DPI-specific square icon sizes (16..256px)"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source-ico", required=True, type=Path)
    parser.add_argument("--window-pid", type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = {"passed": False}
    output = None
    try:
        binary, source = args.binary.resolve(strict=True), args.source_ico.resolve(strict=True)
        require(binary.is_file() and source.is_file(), "Binary and source ICO must be regular files")
        if args.output:
            candidate = args.output.resolve()
            require(candidate not in (binary, source)
                    and not (candidate.exists() and any(os.path.samefile(candidate, item) for item in (binary, source))),
                    "Report output must not overwrite the verified binary or source ICO")
            output = candidate
        frames, sha = source_frames(source)
        api = WinAPI()
        report.update(binary_sha256=file_digest(binary), source_ico_sha256=sha)
        report["resources"] = api.verify_resources(binary, frames)
        report["shell"] = api.verify_shell(binary)
        if args.window_pid is not None:
            require(0 < args.window_pid <= 0xFFFFFFFF, "Window PID must be a positive DWORD")
            report["window"] = api.verify_windows(args.window_pid, binary)
        report["passed"] = True
    except (VerificationError, OSError, ValueError, struct.error) as error:
        report["error"] = str(error)
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
