#!/usr/bin/env python3
"""
Cross-language verification script for D00 deterministic `observation_id` and `sha256-jcs-v1`
payload_hash against Rust-generated `test_vectors.json`.
"""

import base64
import hashlib
import json
import math
import pathlib
import sys


def derive_observation_id_py(
    tenant_id: str,
    source_id: str,
    source_epoch: str,
    message_id: str,
    property_name: str,
) -> str:
    raw = (
        b"iotfast.obs.v1\x00"
        + tenant_id.encode("utf-8")
        + b"\x00"
        + source_id.encode("utf-8")
        + b"\x00"
        + source_epoch.encode("utf-8")
        + b"\x00"
        + message_id.encode("utf-8")
        + b"\x00"
        + property_name.encode("utf-8")
    )
    digest = hashlib.sha256(raw).digest()[:16]
    b32_lower_nopad = base64.b32encode(digest).decode("ascii").rstrip("=").lower()
    return f"obs-{b32_lower_nopad}"


def format_jcs_f64_py(val: float) -> str:
    if math.isnan(val) or math.isinf(val):
        raise ValueError("NaN or Infinity is not allowed in JCS")
    if val == 0.0:
        return "0"

    negative = math.copysign(1.0, val) < 0.0
    abs_val = abs(val)
    # Python's repr(float) produces shortest round-trip decimal digits (David Gay / Burger-Dybvig)
    s = repr(abs_val).lower()
    if "e" in s:
        mantissa_str, exp_str = s.split("e", 1)
        exp_part = int(exp_str)
    else:
        mantissa_str, exp_part = s, 0

    raw_digits = []
    frac_len = 0
    seen_dot = False
    for ch in mantissa_str:
        if ch == ".":
            seen_dot = True
        else:
            raw_digits.append(ch)
            if seen_dot:
                frac_len += 1

    raw_str = "".join(raw_digits)
    trimmed_lead = raw_str.lstrip("0")
    leading_zeros = len(raw_str) - len(trimmed_lead)
    digits = trimmed_lead.rstrip("0")
    k = len(digits)

    int_digits_in_mantissa = len(raw_str) - frac_len
    n_exp = int_digits_in_mantissa - leading_zeros + exp_part

    prefix = "-" if negative else ""
    if k <= n_exp <= 21:
        return prefix + digits + ("0" * (n_exp - k))
    elif 0 < n_exp < k and n_exp <= 21:
        return prefix + digits[:n_exp] + "." + digits[n_exp:]
    elif -5 <= n_exp <= 0:
        return prefix + "0." + ("0" * (-n_exp)) + digits
    else:
        e = n_exp - 1
        sign_e = "+" if e >= 0 else ""
        mant = digits[0] if k == 1 else f"{digits[0]}.{digits[1:]}"
        return f"{prefix}{mant}e{sign_e}{e}"


def format_jcs_string_py(s: str) -> str:
    out = ['"']
    for ch in s:
        code = ord(ch)
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif code == 0x08:
            out.append("\\b")
        elif code == 0x0C:
            out.append("\\f")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif code < 0x20:
            out.append(f"\\u{code:04x}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def format_jcs_opt_str_py(val) -> str:
    if val is None:
        return "null"
    return format_jcs_string_py(val)


def format_typed_value_jcs_py(value_type: str, value) -> str:
    if value_type == "bool":
        return "true" if bool(value) else "false"
    elif value_type == "int64":
        return str(int(value))
    elif value_type == "float64":
        return format_jcs_f64_py(float(value))
    elif value_type == "text":
        return format_jcs_string_py(str(value))
    else:
        raise ValueError(f"Unsupported value_type: {value_type}")


def compute_jcs_v1_payload_hash_py(vec: dict) -> tuple[str, str]:
    # Sort points by property UTF-8 bytes ascending
    sorted_pts = sorted(vec["points"], key=lambda p: p["property"].encode("utf-8"))
    pts_jcs = []
    for pt in sorted_pts:
        val_jcs = format_typed_value_jcs_py(pt["value_type"], pt["value"])
        pt_str = (
            "{"
            f'"property":{format_jcs_string_py(pt["property"])},'
            f'"quality":{{'
            f'"source_code":{format_jcs_opt_str_py(pt["quality_source_code"])},'
            f'"source_protocol":{format_jcs_opt_str_py(pt["quality_source_protocol"])},'
            f'"state":{format_jcs_string_py(pt["quality_state"])}'
            f"}},\"source_at_ms\":{int(pt['source_at_ms'])},"
            f'"source_seq":{format_jcs_string_py(pt["source_seq"])},'
            f'"source_time_origin":{format_jcs_string_py(pt["source_time_origin"])},'
            f'"value":{val_jcs},'
            f'"value_type":{format_jcs_string_py(pt["value_type"])}'
            "}"
        )
        pts_jcs.append(pt_str)

    canonical_json = (
        "{"
        f'"binding_revision_id":{format_jcs_string_py(vec["binding_revision_id"])},'
        f'"device_id":{format_jcs_string_py(vec["device_id"])},'
        f'"model_revision_id":{format_jcs_string_py(vec["model_revision_id"])},'
        f'"points":[{",".join(pts_jcs)}],'
        f'"source_epoch":{format_jcs_string_py(vec["source_epoch"])},'
        f'"source_id":{format_jcs_string_py(vec["source_id"])},'
        f'"spec":{format_jcs_string_py(vec["spec"])},'
        f'"tenant_id":{format_jcs_string_py(vec["tenant_id"])}'
        "}"
    )
    digest_hex = hashlib.sha256(canonical_json.encode("utf-8")).hexdigest()
    return canonical_json, f"sha256:{digest_hex}"


def main() -> int:
    vectors_path = pathlib.Path(__file__).resolve().parent / "test_vectors.json"
    if not vectors_path.exists():
        print(f"ERROR: {vectors_path} does not exist", file=sys.stderr)
        return 1

    suite = json.loads(vectors_path.read_text(encoding="utf-8"))
    vectors = suite["vectors"]
    by_id = {}

    total_obs_checked = 0
    for vec in vectors:
        vid = vec["vector_id"]
        by_id[vid] = vec
        canon_py, hash_py = compute_jcs_v1_payload_hash_py(vec)

        if canon_py != vec["expected_canonical_jcs"]:
            print(f"MISMATCH in canonical_jcs for {vid}:", file=sys.stderr)
            print(f"  Rust:   {vec['expected_canonical_jcs']}", file=sys.stderr)
            print(f"  Python: {canon_py}", file=sys.stderr)
            return 1

        if hash_py != vec["expected_payload_hash"]:
            print(f"MISMATCH in payload_hash for {vid}:", file=sys.stderr)
            print(f"  Rust:   {vec['expected_payload_hash']}", file=sys.stderr)
            print(f"  Python: {hash_py}", file=sys.stderr)
            return 1

        for pt in vec["points"]:
            prop = pt["property"]
            obs_py = derive_observation_id_py(
                vec["tenant_id"],
                vec["source_id"],
                vec["source_epoch"],
                vec["message_id"],
                prop,
            )
            obs_rust = vec["expected_observation_ids"][prop]
            if obs_py != obs_rust:
                print(f"MISMATCH in observation_id for {vid}.{prop}:", file=sys.stderr)
                print(f"  Rust:   {obs_rust}", file=sys.stderr)
                print(f"  Python: {obs_py}", file=sys.stderr)
                return 1
            total_obs_checked += 1

    # Verify cross-vector invariance guarantees
    assert (
        by_id["GV-01-BASELINE-T01"]["expected_payload_hash"]
        == by_id["GV-02-REPLAY-INVARIANCE-SC04"]["expected_payload_hash"]
    ), "GV-01 and GV-02 must have identical payload_hash when replayed toggles false->true"
    assert (
        by_id["GV-01-BASELINE-T01"]["expected_observation_ids"]
        == by_id["GV-02-REPLAY-INVARIANCE-SC04"]["expected_observation_ids"]
    ), "GV-01 and GV-02 must have identical observation_id"
    assert (
        by_id["GV-01-BASELINE-T01"]["expected_payload_hash"]
        != by_id["GV-03-TAMPERED-VALUE-SC05"]["expected_payload_hash"]
    ), "GV-01 and GV-03 must have different payload_hash when value changes 245.6->245.7"
    assert (
        by_id["GV-04-MULTIPOINT-ORDER-A"]["expected_payload_hash"]
        == by_id["GV-05-MULTIPOINT-ORDER-B"]["expected_payload_hash"]
    ), "GV-04 and GV-05 must have identical payload_hash regardless of input points order"
    assert (
        by_id["GV-07-PR2-PROBE-SAMPLE"]["expected_observation_ids"]["outlet_pressure"]
        == "obs-mfwh4ejs2b7rtlcyzrqkyufufa"
    ), "GV-07 PR #2 probe observation_id must match exact expected value"
    assert (
        by_id["GV-07-PR2-PROBE-SAMPLE"]["expected_payload_hash"]
        == "sha256:8d7e884478d3420105475344f4b63187e937741eb68df08832b39b47e8958eae"
    ), "GV-07 PR #2 probe payload_hash must match exact expected value"
    assert (
        len(by_id["GV-07-PR2-PROBE-SAMPLE"]["expected_canonical_jcs"].encode("utf-8")) == 434
    ), "GV-07 PR #2 probe canonical JCS byte length must be 434" 

    print(
        f"OK: Cross-verified {len(vectors)} golden vectors and {total_obs_checked} point observation_ids between Rust and Python 3 (100% match)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
