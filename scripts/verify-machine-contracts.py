#!/usr/bin/env python3
"""
D00.3 Machine Contract & OpenAPI / JSON Schema Verification Script.
Uses ONLY the Python 3 standard library to:
1. Validate JSON syntax, strict duplicate-key rejection (Q03), and schema structure of:
   - docs/specifications/schemas/telemetry-v1.schema.json
   - docs/specifications/schemas/source-state-v1.schema.json
   - docs/specifications/schemas/device-view-v1.schema.json
   - docs/specifications/schemas/history-query-v1.schema.json
   - docs/specifications/openapi-v1.json
2. Verify OpenAPI 3.1 endpoint coverage and $ref integrity against iotfast_server routes.
3. Validate golden vectors from prototype/feasibility/f1_d00_contract/vectors/d00_golden_vectors.json
   (and prototype/feasibility/f1_d00_contract/test_vectors.json) against the machine schemas.
4. Execute positive and negative test cases covering Q01-Q12 (bare JSON values, unknown field
   rejection, duplicate key rejection, southbound source_time_origin enum, source_state v1 required
   fields, device view states, history query 8 required fields, dual-dimension ingestion status,
   and WS subscription_terminated vs resync_required frames).
"""

import copy
import json
import math
import pathlib
import re
import sys
from typing import Any, Dict, List, Optional, Tuple


REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
SCHEMAS_DIR = REPO_ROOT / "docs" / "specifications" / "schemas"
OPENAPI_PATH = REPO_ROOT / "docs" / "specifications" / "openapi-v1.json"

SCHEMA_FILES = {
    "telemetry": SCHEMAS_DIR / "telemetry-v1.schema.json",
    "source_state": SCHEMAS_DIR / "source-state-v1.schema.json",
    "device_view": SCHEMAS_DIR / "device-view-v1.schema.json",
    "history_query": SCHEMAS_DIR / "history-query-v1.schema.json",
}


class DuplicateKeyError(ValueError):
    """Raised when a JSON object contains duplicate keys (forbidden by Q03)."""


def parse_json_strict_no_dup_keys(raw_text: str) -> Any:
    def check_dups(pairs: List[Tuple[str, Any]]) -> Dict[str, Any]:
        obj: Dict[str, Any] = {}
        for k, v in pairs:
            if k in obj:
                raise DuplicateKeyError(f"Duplicate JSON object key '{k}' is forbidden (Q03)")
            obj[k] = v
        return obj

    def reject_non_finite(token: str) -> Any:
        raise ValueError(f"Non-finite JSON constant '{token}' is forbidden (Q03)")

    return json.loads(
        raw_text,
        object_pairs_hook=check_dups,
        parse_constant=reject_non_finite,
    )


class SchemaValidationError(Exception):
    def __init__(self, path: str, message: str) -> None:
        super().__init__(f"{path}: {message}")
        self.path = path
        self.message = message


class StdlibJsonSchemaValidator:
    """
    Lightweight JSON Schema Draft 2020-12 validator using only Python 3 standard library.
    Supports: $ref (internal #/... and relative ./schemas/...), type, const, enum, required,
    properties, additionalProperties (bool or schema), minLength, maxLength, pattern,
    minimum, maximum, minItems, maxItems, items, oneOf.
    """

    def __init__(self, schemas_by_path: Dict[pathlib.Path, Dict[str, Any]]) -> None:
        self.schemas_by_path = {p.resolve(): s for p, s in schemas_by_path.items()}

    def resolve_ref(
        self, ref: str, current_doc_path: pathlib.Path, current_root: Dict[str, Any]
    ) -> Tuple[pathlib.Path, Dict[str, Any], Dict[str, Any]]:
        if "#" in ref:
            file_part, pointer_part = ref.split("#", 1)
        else:
            file_part, pointer_part = ref, ""

        if file_part:
            target_path = (current_doc_path.parent / file_part).resolve()
            if target_path not in self.schemas_by_path:
                raise SchemaValidationError("$ref", f"Unresolved schema file reference: {ref}")
            target_root = self.schemas_by_path[target_path]
        else:
            target_path = current_doc_path
            target_root = current_root

        node: Any = target_root
        if pointer_part:
            if not pointer_part.startswith("/"):
                raise SchemaValidationError("$ref", f"Invalid JSON pointer in $ref: {ref}")
            for token in pointer_part.lstrip("/").split("/"):
                token = token.replace("~1", "/").replace("~0", "~")
                if not isinstance(node, dict) or token not in node:
                    raise SchemaValidationError(
                        "$ref", f"Unresolved JSON pointer '{pointer_part}' in {target_path.name}"
                    )
                node = node[token]

        if not isinstance(node, dict):
            raise SchemaValidationError("$ref", f"Resolved $ref '{ref}' is not a schema object")
        return target_path, target_root, node

    def _matches_single_type(self, instance: Any, expected_type: str) -> bool:
        if expected_type == "null":
            return instance is None
        if expected_type == "boolean":
            return isinstance(instance, bool)
        if expected_type == "integer":
            return isinstance(instance, int) and not isinstance(instance, bool)
        if expected_type == "number":
            if isinstance(instance, bool):
                return False
            if isinstance(instance, (int, float)):
                return math.isfinite(float(instance))
            return False
        if expected_type == "string":
            return isinstance(instance, str)
        if expected_type == "array":
            return isinstance(instance, list)
        if expected_type == "object":
            return isinstance(instance, dict)
        raise SchemaValidationError("$schema", f"Unsupported schema type '{expected_type}'")

    def validate(
        self,
        instance: Any,
        schema: Dict[str, Any],
        doc_path: pathlib.Path,
        root_schema: Optional[Dict[str, Any]] = None,
        instance_path: str = "$",
    ) -> None:
        if root_schema is None:
            root_schema = schema

        if "$ref" in schema:
            target_path, target_root, target_schema = self.resolve_ref(
                schema["$ref"], doc_path, root_schema
            )
            self.validate(instance, target_schema, target_path, target_root, instance_path)
            return

        if "oneOf" in schema:
            matched_count = 0
            last_err: Optional[Exception] = None
            for sub in schema["oneOf"]:
                try:
                    self.validate(instance, sub, doc_path, root_schema, instance_path)
                    matched_count += 1
                except SchemaValidationError as e:
                    last_err = e
            if matched_count != 1:
                raise SchemaValidationError(
                    instance_path,
                    f"Value matched {matched_count} schemas in oneOf (expected 1); last error: {last_err}",
                )

        if "type" in schema:
            expected_types = (
                schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
            )
            if not any(self._matches_single_type(instance, t) for t in expected_types):
                raise SchemaValidationError(
                    instance_path,
                    f"Type mismatch: got {type(instance).__name__} ({instance!r}), expected {expected_types}",
                )

        if "const" in schema:
            if instance != schema["const"] or type(instance) is not type(schema["const"]):
                raise SchemaValidationError(
                    instance_path,
                    f"Const mismatch: got {instance!r}, expected {schema['const']!r}",
                )

        if "enum" in schema:
            enum_matched = False
            for candidate in schema["enum"]:
                if candidate is None and instance is None:
                    enum_matched = True
                    break
                if (
                    candidate is not None
                    and instance is not None
                    and type(candidate) is type(instance)
                    and candidate == instance
                ):
                    enum_matched = True
                    break
            if not enum_matched:
                raise SchemaValidationError(
                    instance_path,
                    f"Value {instance!r} not in allowed enum {schema['enum']}",
                )

        if isinstance(instance, str):
            if "minLength" in schema and len(instance) < schema["minLength"]:
                raise SchemaValidationError(
                    instance_path,
                    f"String length {len(instance)} < minLength {schema['minLength']}",
                )
            if "maxLength" in schema and len(instance) > schema["maxLength"]:
                raise SchemaValidationError(
                    instance_path,
                    f"String length {len(instance)} > maxLength {schema['maxLength']}",
                )
            if "pattern" in schema:
                if re.search(schema["pattern"], instance) is None:
                    raise SchemaValidationError(
                        instance_path,
                        f"String {instance!r} does not match pattern {schema['pattern']!r}",
                    )

        if isinstance(instance, (int, float)) and not isinstance(instance, bool):
            if not math.isfinite(float(instance)):
                raise SchemaValidationError(instance_path, "Non-finite number is forbidden")
            if "minimum" in schema and instance < schema["minimum"]:
                raise SchemaValidationError(
                    instance_path, f"Number {instance} < minimum {schema['minimum']}"
                )
            if "maximum" in schema and instance > schema["maximum"]:
                raise SchemaValidationError(
                    instance_path, f"Number {instance} > maximum {schema['maximum']}"
                )

        if isinstance(instance, list):
            if "minItems" in schema and len(instance) < schema["minItems"]:
                raise SchemaValidationError(
                    instance_path,
                    f"Array length {len(instance)} < minItems {schema['minItems']}",
                )
            if "maxItems" in schema and len(instance) > schema["maxItems"]:
                raise SchemaValidationError(
                    instance_path,
                    f"Array length {len(instance)} > maxItems {schema['maxItems']}",
                )
            if "items" in schema:
                item_schema = schema["items"]
                for idx, elem in enumerate(instance):
                    self.validate(
                        elem,
                        item_schema,
                        doc_path,
                        root_schema,
                        f"{instance_path}[{idx}]",
                    )

        if isinstance(instance, dict):
            req_fields = schema.get("required", [])
            for req_key in req_fields:
                if req_key not in instance:
                    raise SchemaValidationError(
                        instance_path, f"Missing required property '{req_key}'"
                    )

            props = schema.get("properties", {})
            add_props = schema.get("additionalProperties", True)

            for k, v in instance.items():
                child_path = f"{instance_path}.{k}"
                if k in props:
                    self.validate(v, props[k], doc_path, root_schema, child_path)
                else:
                    if add_props is False:
                        raise SchemaValidationError(
                            child_path, f"Unknown property '{k}' is forbidden (additionalProperties=false)"
                        )
                    elif isinstance(add_props, dict):
                        self.validate(v, add_props, doc_path, root_schema, child_path)


def vector_case_to_telemetry_wire(vec: Dict[str, Any]) -> Dict[str, Any]:
    """
    Converts a golden vector entry from d00_golden_vectors.json / test_vectors.json
    into its wire TelemetryEnvelope JSON structure if not already nested.
    """
    for key in ("envelope", "wire_payload", "payload", "input"):
        if key in vec and isinstance(vec[key], dict) and "points" in vec[key]:
            return copy.deepcopy(vec[key])

    points_wire = []
    for pt in vec["points"]:
        if "quality" in pt and isinstance(pt["quality"], dict):
            q_obj = copy.deepcopy(pt["quality"])
        else:
            q_obj = {
                "state": pt["quality_state"],
                "source_protocol": pt.get("quality_source_protocol"),
                "source_code": pt.get("quality_source_code"),
            }
        if "delivery" in pt and isinstance(pt["delivery"], dict):
            d_obj = copy.deepcopy(pt["delivery"])
        else:
            d_obj = {"replayed": pt.get("delivery_replayed", False)}

        points_wire.append(
            {
                "property": pt["property"],
                "value": pt["value"],
                "source_at_ms": pt["source_at_ms"],
                "source_time_origin": pt["source_time_origin"],
                "source_seq": pt["source_seq"],
                "quality": q_obj,
                "delivery": d_obj,
            }
        )

    return {
        "spec": vec.get("spec", "iotfast.telemetry.v1"),
        "message_id": vec["message_id"],
        "source_id": vec["source_id"],
        "source_epoch": vec["source_epoch"],
        "device_id": vec["device_id"],
        "model_revision_id": vec["model_revision_id"],
        "binding_revision_id": vec["binding_revision_id"],
        "points": points_wire,
    }


def vector_case_to_source_state_wire(ss_vec: Dict[str, Any]) -> Dict[str, Any]:
    for key in ("envelope", "wire_payload", "payload", "input"):
        if key in ss_vec and isinstance(ss_vec[key], dict) and "point_states" in ss_vec[key]:
            return copy.deepcopy(ss_vec[key])

    return {
        "spec": ss_vec.get("spec", "iotfast.source_state.v1"),
        "event_id": ss_vec["event_id"],
        "source_id": ss_vec["source_id"],
        "source_epoch": ss_vec["source_epoch"],
        "device_id": ss_vec["device_id"],
        "binding_revision_id": ss_vec["binding_revision_id"],
        "observed_at_ms": ss_vec["observed_at_ms"],
        "source_time_origin": ss_vec["source_time_origin"],
        "source_seq": ss_vec["source_seq"],
        "connection_state": ss_vec["connection_state"],
        "point_states": ss_vec["point_states"],
        "delivery": ss_vec.get(
            "delivery", {"replayed": ss_vec.get("delivery_replayed", False)}
        ),
    }


def verify_openapi_contract(
    openapi_doc: Dict[str, Any],
    validator: StdlibJsonSchemaValidator,
) -> List[str]:
    checks: List[str] = []
    assert openapi_doc.get("openapi") == "3.1.0", "OpenAPI version must be 3.1.0"
    paths = openapi_doc.get("paths", {})

    expected_routes = [
        ("GET", "/health"),
        ("GET", "/v1/health"),
        ("GET", "/healthz"),
        ("GET", "/readyz"),
        ("POST", "/v1/products"),
        ("GET", "/v1/products"),
        ("POST", "/v1/products/{product_id}/models"),
        ("POST", "/v1/models"),
        ("GET", "/v1/models"),
        ("POST", "/v1/models:compare"),
        ("POST", "/v1/models/{model_revision_id}:validate"),
        ("POST", "/v1/models/{model_revision_id}:publish"),
        ("GET", "/v1/models/{model_revision_id}"),
        ("GET", "/v1/models/{model_revision_id}:diff"),
        ("POST", "/v1/assets"),
        ("GET", "/v1/assets"),
        ("POST", "/v1/assets/relations"),
        ("GET", "/v1/assets/{asset_id}/relations"),
        ("POST", "/v1/asset-relations"),
        ("GET", "/v1/asset-relations"),
        ("POST", "/v1/devices"),
        ("GET", "/v1/devices"),
        ("GET", "/v1/devices/{device_id}"),
        ("POST", "/v1/devices/{device_id}:relocate"),
        ("POST", "/v1/devices/{device_id}:upgrade-model"),
        ("GET", "/v1/devices/{device_id}/view"),
        ("GET", "/v1/devices/{device_id}/shadow"),
        ("POST", "/v1/devices/{device_id}/bindings"),
        ("GET", "/v1/resources"),
        ("GET", "/v1/resources:resolve"),
        ("POST", "/v1/sources/grants"),
        ("POST", "/v1/bindings"),
        ("GET", "/v1/bindings/{binding_revision_id}"),
        ("POST", "/v1/bindings/{binding_revision_id}:validate"),
        ("POST", "/v1/bindings/{binding_revision_id}:activate"),
        ("POST", "/v1/telemetry"),
        ("POST", "/v1/source-state"),
        ("GET", "/v1/ingestions/{ingest_id}"),
        ("POST", "/v1/history/query"),
        ("GET", "/v1/subscriptions"),
        ("GET", "/v1/storage/status"),
        ("POST", "/v1/storage/timeseries-adapter"),
        ("POST", "/v1/storage/query-plan"),
        ("GET", "/v1/storage/capabilities"),
        ("GET", "/v1/storage/checkpoints"),
        ("POST", "/v1/storage/checkpoints:reconcile"),
        ("POST", "/v1/storage/backups"),
        ("GET", "/v1/storage/backups"),
        ("POST", "/v1/storage/backups/{backup_id}:verify"),
        ("POST", "/v1/adapters"),
        ("GET", "/v1/adapters"),
        ("POST", "/v1/adapters/{adapter_id}:validate-binding"),
        ("POST", "/v1/derived-metrics"),
        ("GET", "/v1/derived-metrics"),
        ("POST", "/v1/derived-metrics/{metric_id}:evaluate"),
        ("GET", "/v1/ping"),
        ("GET", "/v1/ingest/ping"),
    ]

    for method, route in expected_routes:
        if route not in paths:
            raise AssertionError(f"OpenAPI missing route '{route}'")
        if method.lower() not in paths[route]:
            raise AssertionError(f"OpenAPI route '{route}' missing HTTP method '{method}'")
        checks.append(f"{method} {route}")

    def walk_refs(node: Any) -> int:
        count = 0
        if isinstance(node, dict):
            if "$ref" in node and isinstance(node["$ref"], str):
                validator.resolve_ref(node["$ref"], OPENAPI_PATH.resolve(), openapi_doc)
                count += 1
            for v in node.values():
                count += walk_refs(v)
        elif isinstance(node, list):
            for elem in node:
                count += walk_refs(elem)
        return count

    ref_count = walk_refs(openapi_doc)
    checks.append(f"OpenAPI $ref resolution ({ref_count} references verified)")
    return checks


def main() -> int:
    # 1. Load and strictly parse all 4 JSON Schemas + OpenAPI 3.1
    loaded_docs: Dict[pathlib.Path, Dict[str, Any]] = {}
    for name, path in list(SCHEMA_FILES.items()) + [("openapi", OPENAPI_PATH)]:
        if not path.exists():
            print(f"ERROR: Missing required contract file: {path}", file=sys.stderr)
            return 1
        raw = path.read_text(encoding="utf-8")
        doc = parse_json_strict_no_dup_keys(raw)
        if not isinstance(doc, dict):
            print(f"ERROR: Contract root in {path} is not a JSON object", file=sys.stderr)
            return 1
        loaded_docs[path.resolve()] = doc

    validator = StdlibJsonSchemaValidator(loaded_docs)

    telemetry_path = SCHEMA_FILES["telemetry"].resolve()
    telemetry_schema = loaded_docs[telemetry_path]

    source_state_path = SCHEMA_FILES["source_state"].resolve()
    source_state_schema = loaded_docs[source_state_path]

    device_view_path = SCHEMA_FILES["device_view"].resolve()
    device_view_schema = loaded_docs[device_view_path]

    history_query_path = SCHEMA_FILES["history_query"].resolve()
    history_query_schema = loaded_docs[history_query_path]

    openapi_path = OPENAPI_PATH.resolve()
    openapi_doc = loaded_docs[openapi_path]

    # 2. Verify OpenAPI endpoints and $ref resolution
    openapi_checks = verify_openapi_contract(openapi_doc, validator)

    results: List[Tuple[str, str, str]] = []

    def expect_valid(
        case_id: str,
        instance: Any,
        schema: Dict[str, Any],
        doc_path: pathlib.Path,
        root_schema: Optional[Dict[str, Any]] = None,
    ) -> None:
        validator.validate(instance, schema, doc_path, root_schema or schema)
        results.append((case_id, "POSITIVE (EXPECT VALID)", "PASS"))

    def expect_invalid(
        case_id: str,
        instance: Any,
        schema: Dict[str, Any],
        doc_path: pathlib.Path,
        expected_substr: str,
        root_schema: Optional[Dict[str, Any]] = None,
    ) -> None:
        try:
            validator.validate(instance, schema, doc_path, root_schema or schema)
        except SchemaValidationError as err:
            if expected_substr not in str(err):
                raise AssertionError(
                    f"Case {case_id} failed as expected, but error '{err}' did not contain '{expected_substr}'"
                ) from err
            results.append((case_id, f"NEGATIVE (REJECT: {expected_substr})", "PASS"))
            return
        raise AssertionError(f"Case {case_id} unexpectedly PASSED validation (expected failure)")

    # 3. Validate golden vectors from d00_golden_vectors.json and/or test_vectors.json
    vector_paths = [
        REPO_ROOT
        / "prototype"
        / "feasibility"
        / "f1_d00_contract"
        / "vectors"
        / "d00_golden_vectors.json",
        REPO_ROOT / "prototype" / "feasibility" / "f1_d00_contract" / "test_vectors.json",
    ]
    existing_vector_paths = [p for p in vector_paths if p.exists()]
    if not existing_vector_paths:
        print("ERROR: Neither d00_golden_vectors.json nor test_vectors.json found", file=sys.stderr)
        return 1

    golden_checked = 0
    for vp in existing_vector_paths:
        suite = parse_json_strict_no_dup_keys(vp.read_text(encoding="utf-8"))
        for list_key in ("vectors", "telemetry_vectors"):
            for vec in suite.get(list_key, []):
                if not isinstance(vec, dict):
                    continue
                if vec.get("expected_valid") is False:
                    continue
                vid = f"{vp.name}:{vec.get('vector_id', 'vec')}"
                spec_val = vec.get("spec", "iotfast.telemetry.v1")
                if spec_val == "iotfast.telemetry.v1" and "points" in vec or "envelope" in vec or "wire_payload" in vec:
                    if spec_val == "iotfast.source_state.v1":
                        wire = vector_case_to_source_state_wire(vec)
                        expect_valid(vid, wire, source_state_schema, source_state_path)
                    else:
                        wire = vector_case_to_telemetry_wire(vec)
                        expect_valid(vid, wire, telemetry_schema, telemetry_path)
                    golden_checked += 1
                elif spec_val == "iotfast.source_state.v1" and "point_states" in vec:
                    wire = vector_case_to_source_state_wire(vec)
                    expect_valid(vid, wire, source_state_schema, source_state_path)
                    golden_checked += 1

        for ss_vec in suite.get("source_state_vectors", []):
            if not isinstance(ss_vec, dict) or ss_vec.get("expected_valid") is False:
                continue
            vid = f"{vp.name}:{ss_vec.get('vector_id', 'ss_vec')}"
            wire = vector_case_to_source_state_wire(ss_vec)
            expect_valid(vid, wire, source_state_schema, source_state_path)
            golden_checked += 1

    # 4. Construct baseline valid telemetry & run negative telemetry tests (Q03, SC09)
    base_telemetry = {
        "spec": "iotfast.telemetry.v1",
        "message_id": "m-20261105-000001",
        "source_id": "gateway-01",
        "source_epoch": "epoch-001",
        "device_id": "dev-oven-02",
        "model_revision_id": "mr-oven-1",
        "binding_revision_id": "br-oven-12",
        "points": [
            {
                "property": "zone1_temp",
                "value": 245.6,
                "source_at_ms": 1793869200120,
                "source_time_origin": "gateway_sample",
                "source_seq": "1001",
                "quality": {
                    "state": "good",
                    "source_protocol": "modbus",
                    "source_code": None,
                },
                "delivery": {
                    "replayed": False,
                },
            }
        ],
    }
    expect_valid("POS-TEL-BASELINE", base_telemetry, telemetry_schema, telemetry_path)

    # NEG-TEL-01: Unknown top-level field
    neg_tel_01 = copy.deepcopy(base_telemetry)
    neg_tel_01["unknown_top_field"] = "forbidden"
    expect_invalid(
        "NEG-TEL-01-UNKNOWN-TOP-FIELD",
        neg_tel_01,
        telemetry_schema,
        telemetry_path,
        "Unknown property 'unknown_top_field' is forbidden",
    )

    # NEG-TEL-02: Unknown nested field in point
    neg_tel_02 = copy.deepcopy(base_telemetry)
    neg_tel_02["points"][0]["unit"] = "Cel"
    expect_invalid(
        "NEG-TEL-02-UNKNOWN-POINT-FIELD",
        neg_tel_02,
        telemetry_schema,
        telemetry_path,
        "Unknown property 'unit' is forbidden",
    )

    # NEG-TEL-03: Duplicate JSON key on wire (Q03)
    dup_json_wire = (
        '{"spec":"iotfast.telemetry.v1","spec":"iotfast.telemetry.v1",'
        '"message_id":"m-1","source_id":"gateway-01"}'
    )
    try:
        parse_json_strict_no_dup_keys(dup_json_wire)
        raise AssertionError("NEG-TEL-03-DUPLICATE-KEY unexpectedly succeeded")
    except DuplicateKeyError:
        results.append(("NEG-TEL-03-DUPLICATE-JSON-KEY", "NEGATIVE (REJECT: Duplicate key)", "PASS"))

    # NEG-TEL-04: Enum-wrapped TypedPointValue object instead of bare scalar (Q03)
    neg_tel_04 = copy.deepcopy(base_telemetry)
    neg_tel_04["points"][0]["value"] = {"Float64": 245.6}
    expect_invalid(
        "NEG-TEL-04-ENUM-WRAPPED-VALUE",
        neg_tel_04,
        telemetry_schema,
        telemetry_path,
        "Type mismatch",
    )

    # NEG-TEL-05: Null value in telemetry point (SC09)
    neg_tel_05 = copy.deepcopy(base_telemetry)
    neg_tel_05["points"][0]["value"] = None
    expect_invalid(
        "NEG-TEL-05-NULL-POINT-VALUE",
        neg_tel_05,
        telemetry_schema,
        telemetry_path,
        "Type mismatch",
    )

    # NEG-TEL-06: Southbound telemetry with external_source time origin (Q03)
    neg_tel_06 = copy.deepcopy(base_telemetry)
    neg_tel_06["points"][0]["source_time_origin"] = "external_source"
    expect_invalid(
        "NEG-TEL-06-EXTERNAL-SOURCE-ORIGIN",
        neg_tel_06,
        telemetry_schema,
        telemetry_path,
        "not in allowed enum",
    )

    # NEG-TEL-07: Leading zero in source_seq
    neg_tel_07 = copy.deepcopy(base_telemetry)
    neg_tel_07["points"][0]["source_seq"] = "01001"
    expect_invalid(
        "NEG-TEL-07-LEADING-ZERO-SEQ",
        neg_tel_07,
        telemetry_schema,
        telemetry_path,
        "does not match pattern",
    )

    # NEG-TEL-08: Missing required field binding_revision_id
    neg_tel_08 = copy.deepcopy(base_telemetry)
    del neg_tel_08["binding_revision_id"]
    expect_invalid(
        "NEG-TEL-08-MISSING-BINDING-REV",
        neg_tel_08,
        telemetry_schema,
        telemetry_path,
        "Missing required property 'binding_revision_id'",
    )

    # 5. Source State v1 positive & negative tests (Q04, C02)
    pos_ss_01 = {
        "spec": "iotfast.source_state.v1",
        "event_id": "evt-20261105-000089",
        "source_id": "gateway-01",
        "source_epoch": "epoch-001",
        "device_id": "dev-oven-02",
        "binding_revision_id": "br-oven-12",
        "observed_at_ms": 1793869260000,
        "source_time_origin": "gateway_sample",
        "source_seq": "1002",
        "connection_state": "disconnected",
        "point_states": [
            {
                "property": "zone1_temp",
                "quality_state": "bad",
                "quality_reason": "modbus_read_timeout",
                "source_protocol": "modbus",
                "source_code": "ETIMEDOUT",
            }
        ],
        "delivery": {
            "replayed": False,
        },
    }
    expect_valid("POS-SS-01-POINT-FAULT", pos_ss_01, source_state_schema, source_state_path)

    pos_ss_02 = copy.deepcopy(pos_ss_01)
    pos_ss_02["event_id"] = "evt-20261105-000090"
    pos_ss_02["point_states"] = []
    expect_valid("POS-SS-02-PURE-DISCONNECT", pos_ss_02, source_state_schema, source_state_path)

    neg_ss_01 = copy.deepcopy(pos_ss_01)
    del neg_ss_01["source_time_origin"]
    expect_invalid(
        "NEG-SS-01-MISSING-TIME-ORIGIN",
        neg_ss_01,
        source_state_schema,
        source_state_path,
        "Missing required property 'source_time_origin'",
    )

    neg_ss_02 = copy.deepcopy(pos_ss_01)
    del neg_ss_02["delivery"]
    expect_invalid(
        "NEG-SS-02-MISSING-DELIVERY",
        neg_ss_02,
        source_state_schema,
        source_state_path,
        "Missing required property 'delivery'",
    )

    neg_ss_03 = copy.deepcopy(pos_ss_01)
    neg_ss_03["connection_state"] = "link_down"
    expect_invalid(
        "NEG-SS-03-INVALID-CONN-STATE",
        neg_ss_03,
        source_state_schema,
        source_state_path,
        "not in allowed enum",
    )

    # 6. DeviceBusinessView positive & negative tests (NB03, NB04, R09)
    pos_view_01 = {
        "request_id": "req-view-01",
        "device_id": "pump-001",
        "device_key": "pump-001",
        "device_name": "1号循环水泵",
        "product_id": "prod-pump",
        "product_key": "circulation-pump",
        "model_revision_id": "pump-model-r1",
        "model_version": "1.0.0",
        "asset_id": "asset-plant-a-room-1",
        "canonical_device_uri": "uns://8iic/plant-a/room-1/pump-001",
        "active_binding_revision_id": "pump-001-bind-r1",
        "source_connection_state": "connected",
        "view_generated_at_ms": 1793869205000,
        "view_consistency": "per_resource_latest",
        "properties": {
            "outlet_pressure": {
                "resource_id": "res-pump-001-outlet-pressure",
                "name": "出口压力",
                "data_type": "float64",
                "unit": {"code": "MPa", "symbol": "MPa"},
                "binding_state": "bound",
                "observation_state": "observed",
                "value": 0.42,
                "quality": "bad",
                "quality_reason": "modbus_read_timeout",
                "freshness": "stale",
                "value_sampled_at_ms": 1793869100120,
                "quality_observed_at_ms": 1793869200000,
                "received_at_ms": 1793869100145,
                "source_time_origin": "gateway_sample",
                "binding_revision_id": "pump-001-bind-r1",
                "resource_revision": 4,
            },
            "rated_power_kw": {
                "resource_id": "res-pump-001-rated-power",
                "name": "额定功率",
                "data_type": "float64",
                "unit_code": "kW",
                "unit_symbol": "kW",
                "binding_state": "bound_static",
                "observation_state": "static_configured",
                "value": 15.0,
                "quality": "good",
                "quality_reason": None,
                "freshness": "static",
                "value_sampled_at_ms": 1793869000000,
                "quality_observed_at_ms": 1793869000000,
                "received_at_ms": 1793869000000,
                "source_time_origin": "external_source",
                "binding_revision_id": "pump-001-bind-r1",
                "resource_revision": 1,
            },
            "maintenance_note": {
                "resource_id": "res-pump-001-maint-note",
                "name": "维护备注",
                "data_type": "text",
                "unit": None,
                "binding_state": "unbound",
                "observation_state": "no_observation",
                "value": None,
                "quality": "unknown",
                "quality_reason": "unbound_property",
                "freshness": "unknown",
                "value_sampled_at_ms": None,
                "quality_observed_at_ms": None,
                "received_at_ms": None,
                "source_time_origin": None,
                "binding_revision_id": None,
                "resource_revision": 0,
            },
        },
    }
    expect_valid("POS-VIEW-01-FULL", pos_view_01, device_view_schema, device_view_path)

    neg_view_01 = copy.deepcopy(pos_view_01)
    neg_view_01["properties"]["outlet_pressure"]["binding_state"] = "auto_bound"
    expect_invalid(
        "NEG-VIEW-01-INVALID-BINDING-STATE",
        neg_view_01,
        device_view_schema,
        device_view_path,
        "not in allowed enum",
    )

    neg_view_02 = copy.deepcopy(pos_view_01)
    neg_view_02["properties"]["maintenance_note"]["observation_state"] = "default_zero"
    expect_invalid(
        "NEG-VIEW-02-INVALID-OBS-STATE",
        neg_view_02,
        device_view_schema,
        device_view_path,
        "not in allowed enum",
    )

    # 7. HistoryQueryRequest (8 required fields) & HistoryQueryResponse (Q08)
    pos_hist_req = {
        "resource_ids": ["res-pump-001-outlet-pressure"],
        "range": {"start_ms": 1793865600000, "end_ms": 1793869200000},
        "mode": "raw",
        "aggregation": None,
        "quality_states": ["good", "uncertain", "bad"],
        "order": "asc",
        "limit": 500,
        "cursor": None,
    }
    expect_valid("POS-HIST-REQ-01", pos_hist_req, history_query_schema, history_query_path)

    for field_name in [
        "resource_ids",
        "range",
        "mode",
        "aggregation",
        "quality_states",
        "order",
        "limit",
        "cursor",
    ]:
        missing_req = copy.deepcopy(pos_hist_req)
        del missing_req[field_name]
        expect_invalid(
            f"NEG-HIST-REQ-MISSING-{field_name.upper()}",
            missing_req,
            history_query_schema,
            history_query_path,
            f"Missing required property '{field_name}'",
        )

    neg_hist_sql = copy.deepcopy(pos_hist_req)
    neg_hist_sql["where_sql"] = "1=1; DROP TABLE resource;"
    expect_invalid(
        "NEG-HIST-REQ-SQL-INJECTION-FIELD",
        neg_hist_sql,
        history_query_schema,
        history_query_path,
        "Unknown property 'where_sql' is forbidden",
    )

    pos_hist_resp = {
        "request_id": "req-hist-01",
        "target_id": "local-duckdb",
        "source_mode": "managed_ingest",
        "mode": "raw",
        "consistency": "strong_snapshot",
        "partial": False,
        "warnings": [],
        "coverage": {
            "contiguous_wal_position": "0000000000000001:00000000000001a0",
            "event_time_complete_through_ms": None,
            "event_time_completeness": "unknown",
            "known_gaps": [],
        },
        "known_gaps": [],
        "items": [
            {
                "observation_id": "obs-hzg4jz6lu5qzfhygt5o3wtyrpi",
                "resource_id": "res-pump-001-outlet-pressure",
                "device_id": "pump-001",
                "property": "outlet_pressure",
                "model_revision_id": "pump-model-r1",
                "binding_revision_id": "pump-001-bind-r1",
                "source_at_ms": 1793865601120,
                "received_at_ms": 1793865601150,
                "source_time_origin": "gateway_sample",
                "source_seq": "1001",
                "value_type": "float64",
                "value": 0.42,
                "unit": {"code": "MPa", "symbol": "MPa"},
                "quality": "good",
                "source_quality_code": None,
                "replayed": False,
                "record_kind": "measured",
            }
        ],
        "buckets": [],
        "next_cursor": None,
    }
    expect_valid(
        "POS-HIST-RESP-01",
        pos_hist_resp,
        history_query_schema["$defs"]["HistoryQueryResponse"],
        history_query_path,
        history_query_schema,
    )

    # 8. Dual-Dimension Ingestion Status (Q12) & WebSocket Termination vs Resync Frames (Q05)
    pos_ing_status = {
        "ingest_id": "ing-000001",
        "tenant_id": "ten-8iic",
        "message_id": "m-20261105-000001",
        "status": "accepted_durable",
        "history_status": "committed",
        "wal_position": "0000000000000001:0000000000000000",
        "updated_at_ms": 1793869200150,
    }
    expect_valid(
        "POS-INGEST-STATUS-DURABLE-Q12",
        pos_ing_status,
        telemetry_schema["$defs"]["IngestStatusRecord"],
        telemetry_path,
        telemetry_schema,
    )

    neg_ing_status = copy.deepcopy(pos_ing_status)
    del neg_ing_status["history_status"]
    expect_invalid(
        "NEG-INGEST-STATUS-MISSING-HISTORY-STATUS-Q12",
        neg_ing_status,
        telemetry_schema["$defs"]["IngestStatusRecord"],
        telemetry_path,
        "Missing required property 'history_status'",
        telemetry_schema,
    )

    ws_term_schema = openapi_doc["components"]["schemas"]["WsSubscriptionTerminatedFrame"]
    ws_resync_schema = openapi_doc["components"]["schemas"]["WsResyncRequiredFrame"]

    pos_ws_term = {
        "type": "subscription_terminated",
        "sub_id": "sub-client-01",
        "server_sub_id": "sub-000001",
        "reason": "CREDENTIAL_EXPIRED",
        "message": "Subscriber scope credential expired (SC10)",
        "action": "reauthenticate_and_resubscribe",
    }
    expect_valid(
        "POS-WS-SUBSCRIPTION-TERMINATED-Q05",
        pos_ws_term,
        ws_term_schema,
        openapi_path,
        openapi_doc,
    )

    pos_ws_resync = {
        "type": "resync_required",
        "sub_id": "sub-client-01",
        "server_sub_id": "sub-000001",
        "reason": "BUFFER_OVERFLOW",
        "message": "Pending delta queue exceeded max_pending_events=64",
        "action": "reconnect_and_resubscribe_with_snapshot",
    }
    expect_valid(
        "POS-WS-RESYNC-REQUIRED-Q05",
        pos_ws_resync,
        ws_resync_schema,
        openapi_path,
        openapi_doc,
    )

    neg_ws_conflated = copy.deepcopy(pos_ws_resync)
    neg_ws_conflated["reason"] = "CREDENTIAL_EXPIRED"
    expect_invalid(
        "NEG-WS-RESYNC-WITH-CREDENTIAL-EXPIRED-Q05",
        neg_ws_conflated,
        ws_resync_schema,
        openapi_path,
        "not in allowed enum",
        openapi_doc,
    )

    # 9. Storage Architecture & Pluggable Time-Series Adapter Schemas
    storage_status_schema = openapi_doc["components"]["schemas"]["StorageArchitectureStatus"]
    cfg_ts_adapter_schema = openapi_doc["components"]["schemas"]["ConfigureTimeSeriesAdapterRequest"]
    build_plan_req_schema = openapi_doc["components"]["schemas"]["BuildQueryPlanRequest"]
    query_plan_schema = openapi_doc["components"]["schemas"]["TimeSeriesQueryPlan"]

    pos_storage_status = {
        "architecture": "sqlite_business_metadata + pluggable_timeseries_adapter",
        "business_metadata_store": {
            "engine": "sqlite",
            "sqlite_version": "3.45.1",
            "db_path": "/var/lib/iotfast/business_metadata.sqlite",
            "journal_mode": "wal",
            "tables": [
                "product",
                "model_revision",
                "model_item",
                "asset",
                "device",
                "resource",
                "resource_path",
                "source_gateway",
                "source_device_grant",
                "binding_revision",
                "point_binding",
                "asset_relation",
            ],
            "row_counts": {"product": 1, "device": 1, "resource": 2},
        },
        "device_timeseries_store": {
            "active_backend": "tdengine",
            "capabilities": {
                "backend": "tdengine",
                "exact_id_roundtrip": "supported",
                "same_timestamp_distinct": "conditional",
                "replay_idempotent": "conditional",
                "duplicate_conflict_detect": "conditional",
                "batch_result_scope": "conditional",
                "snapshot_pagination": "conditional",
                "quality_roundtrip": "supported",
                "notes": [
                    "TDengine STABLE uses (_ts, resource_id) subtable primary key; sub-ms collisions require distinct timestamps or WAL authoritative mirror."
                ],
            },
            "supported_backends": ["duckdb", "tdengine", "timescaledb", "clickhouse"],
            "duckdb_path": "/var/lib/iotfast/engine.duckdb",
            "tdengine_config": {
                "endpoint": "http://127.0.0.1:6041",
                "database": "iotfast_tsdb",
                "super_table": "stb_observations",
                "username": "root",
                "password": "***REDACTED***",
                "execution_mode": "local_mirror_with_sql_spool",
            },
            "tdengine_super_table_ddl": "CREATE DATABASE IF NOT EXISTS iotfast_tsdb PRECISION 'ms' KEEP 365; CREATE STABLE IF NOT EXISTS iotfast_tsdb.stb_observations (...);",
            "recent_tdengine_sql_log": [
                "CREATE DATABASE IF NOT EXISTS iotfast_tsdb PRECISION 'ms' KEEP 365; CREATE STABLE IF NOT EXISTS iotfast_tsdb.stb_observations (...);"
            ],
            "tables": [
                "iotfast_tsdb.stb_observations (STABLE)",
                "latest_value",
                "point_quality_state",
                "history_observation",
            ],
            "row_counts": {"latest_value": 2, "history_observation": 10},
        },
    }
    expect_valid(
        "POS-STORAGE-ARCH-STATUS",
        pos_storage_status,
        storage_status_schema,
        openapi_path,
        openapi_doc,
    )

    pos_cfg_adapter = {
        "backend": "tdengine",
        "tdengine": {
            "endpoint": "http://127.0.0.1:6041",
            "database": "iotfast_tsdb",
            "super_table": "stb_observations",
            "username": "root",
            "password": "taosdata",
            "execution_mode": "local_mirror_with_sql_spool",
        },
    }
    expect_valid(
        "POS-CFG-TS-ADAPTER-TDENGINE",
        pos_cfg_adapter,
        cfg_ts_adapter_schema,
        openapi_path,
        openapi_doc,
    )

    neg_cfg_adapter = {"backend": "influxdb_v1"}
    expect_invalid(
        "NEG-CFG-TS-ADAPTER-UNSUPPORTED-BACKEND",
        neg_cfg_adapter,
        cfg_ts_adapter_schema,
        openapi_path,
        "not in allowed enum",
        openapi_doc,
    )

    pos_build_plan_req = {
        "resource_ids": ["res-pump-001-outlet-pressure"],
        "start_ms": 10000,
        "end_ms": 70000,
        "bucket_ms": 60000,
    }
    expect_valid(
        "POS-BUILD-QUERY-PLAN-REQ",
        pos_build_plan_req,
        build_plan_req_schema,
        openapi_path,
        openapi_doc,
    )

    pos_query_plan = {
        "backend": "tdengine",
        "mode": "aggregate",
        "native_sql": "SELECT _wstart AS bucket_start_ms, resource_id, AVG(val_f64) AS avg_val FROM iotfast_tsdb.stb_observations PARTITION BY resource_id INTERVAL(60000a) FILL(NONE);",
    }
    expect_valid(
        "POS-TS-QUERY-PLAN-RESP",
        pos_query_plan,
        query_plan_schema,
        openapi_path,
        openapi_doc,
    )

    # 10. Stage 6 Schemas: EdgeAdapterCapability, DerivedMetricDefinition, PingResponse
    adapter_cap_schema = openapi_doc["components"]["schemas"]["EdgeAdapterCapability"]
    val_adapter_req_schema = openapi_doc["components"]["schemas"]["ValidateAdapterBindingRequest"]
    val_adapter_resp_schema = openapi_doc["components"]["schemas"]["ValidateAdapterBindingResponse"]
    derived_metric_schema = openapi_doc["components"]["schemas"]["DerivedMetricDefinition"]
    eval_metric_req_schema = openapi_doc["components"]["schemas"]["EvaluateDerivedMetricRequest"]
    eval_metric_resp_schema = openapi_doc["components"]["schemas"]["EvaluateDerivedMetricResponse"]
    ping_resp_schema = openapi_doc["components"]["schemas"]["PingResponse"]

    pos_adapter_cap = {
        "adapter_id": "adapter-modbus-gw01",
        "tenant_id": "ten-8iic",
        "protocol": "modbus_tcp",
        "supported_value_types": ["float64", "int64", "bool", "text"],
        "supported_transforms": ["gateway_normalized", "linear_scale"],
        "max_batch_size": 500,
        "supports_deadband": True,
        "updated_at_ms": 25500,
    }
    expect_valid(
        "POS-STAGE6-EDGE-ADAPTER-CAP",
        pos_adapter_cap,
        adapter_cap_schema,
        openapi_path,
        openapi_doc,
    )

    pos_val_adapter_req = {
        "value_type": "float64",
        "transform": "gateway_normalized",
        "batch_size": 100,
        "requires_deadband": True,
    }
    expect_valid(
        "POS-STAGE6-VALIDATE-ADAPTER-REQ",
        pos_val_adapter_req,
        val_adapter_req_schema,
        openapi_path,
        openapi_doc,
    )

    pos_val_adapter_resp = {
        "adapter_id": "adapter-modbus-gw01",
        "compatible": True,
        "checked_at_ms": 25600,
    }
    expect_valid(
        "POS-STAGE6-VALIDATE-ADAPTER-RESP",
        pos_val_adapter_resp,
        val_adapter_resp_schema,
        openapi_path,
        openapi_doc,
    )

    pos_derived_metric = {
        "metric_id": "metric-specific-power",
        "tenant_id": "ten-8iic",
        "name": "单位压力功率比",
        "product_id": "prod-pump",
        "model_revision_id": "pump-model-r1",
        "expression": "rated_power / outlet_pressure",
        "input_properties": ["rated_power", "outlet_pressure"],
        "output_unit_code": "kW_per_MPa",
        "quality_policy": "reject_on_bad",
        "created_at_ms": 25600,
    }
    expect_valid(
        "POS-STAGE6-DERIVED-METRIC-DEF",
        pos_derived_metric,
        derived_metric_schema,
        openapi_path,
        openapi_doc,
    )

    neg_derived_metric = copy.deepcopy(pos_derived_metric)
    neg_derived_metric["quality_policy"] = "ignore_bad_silent_zero"
    expect_invalid(
        "NEG-STAGE6-DERIVED-METRIC-INVALID-QUALITY-POLICY",
        neg_derived_metric,
        derived_metric_schema,
        openapi_path,
        "not in allowed enum",
        openapi_doc,
    )

    pos_eval_metric_req = {
        "device_id": "pump-001",
        "evaluated_at_ms": 30500,
    }
    expect_valid(
        "POS-STAGE6-EVAL-DERIVED-METRIC-REQ",
        pos_eval_metric_req,
        eval_metric_req_schema,
        openapi_path,
        openapi_doc,
    )

    pos_eval_metric_resp = {
        "metric_id": "metric-specific-power",
        "device_id": "pump-001",
        "expression": "rated_power / outlet_pressure",
        "computed_value": 33.333333,
        "unit_code": "kW_per_MPa",
        "quality": "good",
        "evaluated_at_ms": 30500,
        "inputs_lineage": {
            "rated_power": {
                "resource_id": "res-pump-001-rated-power",
                "value": 55.0,
                "unit_code": "kW",
                "quality": "good",
                "freshness": "static",
                "binding_state": "bound_static",
            },
            "outlet_pressure": {
                "resource_id": "res-pump-001-outlet-pressure",
                "value": 1.65,
                "unit_code": "MPa",
                "quality": "good",
                "freshness": "fresh",
                "binding_state": "bound",
            },
        },
    }
    expect_valid(
        "POS-STAGE6-EVAL-DERIVED-METRIC-RESP",
        pos_eval_metric_resp,
        eval_metric_resp_schema,
        openapi_path,
        openapi_doc,
    )

    pos_ping_resp = {
        "status": "ok",
        "plane": "southbound_ingest",
        "tenant_id": "ten-8iic",
        "caller_service_id": "gw-01",
        "wal_ready": True,
        "server_time_ms": 30000,
    }
    expect_valid(
        "POS-STAGE6-PING-RESP",
        pos_ping_resp,
        ping_resp_schema,
        openapi_path,
        openapi_doc,
    )

    # 11. ST-02 StorageCapabilitiesResponse & ST-04 DeliveryCheckpoints / ReconcileDeliveryHoleRequest
    storage_caps_schema = openapi_doc["components"]["schemas"]["StorageCapabilitiesResponse"]
    checkpoint_summary_schema = openapi_doc["components"]["schemas"]["DeliveryCheckpointSummary"]
    reconcile_hole_req_schema = openapi_doc["components"]["schemas"]["ReconcileDeliveryHoleRequest"]
    switch_ts_req_schema = openapi_doc["components"]["schemas"]["SwitchTimeSeriesAdapterRequest"]

    pos_storage_caps = {
        "active_backend": "duckdb",
        "active_capabilities": {
            "backend": "duckdb",
            "exact_id_roundtrip": "supported",
            "same_timestamp_distinct": "supported",
            "replay_idempotent": "supported",
            "duplicate_conflict_detect": "supported",
            "batch_result_scope": "supported",
            "snapshot_pagination": "supported",
            "quality_roundtrip": "supported",
            "notes": ["Default embedded lossless OLAP store"],
        },
        "backends": [
            {
                "backend": "duckdb",
                "exact_id_roundtrip": "supported",
                "same_timestamp_distinct": "supported",
                "replay_idempotent": "supported",
                "duplicate_conflict_detect": "supported",
                "batch_result_scope": "supported",
                "snapshot_pagination": "supported",
                "quality_roundtrip": "supported",
                "notes": ["Default embedded lossless OLAP store"],
            }
        ],
    }
    expect_valid(
        "POS-ST02-STORAGE-CAPABILITIES-RESP",
        pos_storage_caps,
        storage_caps_schema,
        openapi_path,
        openapi_doc,
    )

    pos_checkpoint_summary = {
        "target_backend": "duckdb",
        "contiguous_checkpoint_wal_pos": "0000000000000001:0000000000000000",
        "highest_seen_wal_pos": "0000000000000001:0000000000000080",
        "committed_count": 1,
        "isolated_gap_count": 0,
        "unknown_count": 1,
        "retryable_count": 0,
        "permanent_rejected_count": 0,
        "has_unreconciled_holes": True,
        "holes": [
            {
                "wal_position": "0000000000000001:0000000000000080",
                "ingest_id": "ing-000002",
                "target_backend": "duckdb",
                "status": "unknown",
                "reason": "HTTP timeout waiting for remote TSDB acknowledgement",
                "operator_id": None,
                "recorded_at_ms": 28000,
            }
        ],
    }
    expect_valid(
        "POS-ST04-DELIVERY-CHECKPOINT-SUMMARY",
        pos_checkpoint_summary,
        checkpoint_summary_schema,
        openapi_path,
        openapi_doc,
    )

    pos_reconcile_req = {
        "target_backend": "duckdb",
        "wal_position": "0000000000000001:0000000000000000",
        "resolution": "mark_committed",
        "reason": "Verified observation_id roundtrip against target store",
        "operator_id": "op-apifox",
    }
    expect_valid(
        "POS-ST04-RECONCILE-DELIVERY-HOLE-REQ",
        pos_reconcile_req,
        reconcile_hole_req_schema,
        openapi_path,
        openapi_doc,
    )

    neg_reconcile_extra = copy.deepcopy(pos_reconcile_req)
    neg_reconcile_extra["force_skip_audit"] = True
    expect_invalid(
        "NEG-ST04-RECONCILE-UNKNOWN-FIELD",
        neg_reconcile_extra,
        reconcile_hole_req_schema,
        openapi_path,
        "Unknown property 'force_skip_audit' is forbidden",
        openapi_doc,
    )

    neg_reconcile_res = copy.deepcopy(pos_reconcile_req)
    neg_reconcile_res["resolution"] = "silent_drop"
    expect_invalid(
        "NEG-ST04-RECONCILE-INVALID-RESOLUTION",
        neg_reconcile_res,
        reconcile_hole_req_schema,
        openapi_path,
        "not in allowed enum",
        openapi_doc,
    )

    pos_switch_strict = {
        "backend": "tdengine",
        "strict_lossless": True,
    }
    expect_valid(
        "POS-ST02-SWITCH-TS-ADAPTER-STRICT-LOSSLESS",
        pos_switch_strict,
        switch_ts_req_schema,
        openapi_path,
        openapi_doc,
    )

    # 12. D10.1 Snapshot Backups & Isolated Restore Verification Schemas
    create_bkp_req_schema = openapi_doc["components"]["schemas"]["CreateBackupRequest"]
    verify_bkp_req_schema = openapi_doc["components"]["schemas"]["VerifyBackupRequest"]
    bkp_manifest_schema = openapi_doc["components"]["schemas"]["BackupManifest"]
    bkp_list_schema = openapi_doc["components"]["schemas"]["BackupListResponse"]
    bkp_verify_report_schema = openapi_doc["components"]["schemas"]["BackupVerifyReport"]

    pos_create_bkp_req = {"backup_id": "bkp-apifox-001"}
    expect_valid(
        "POS-D10-CREATE-BACKUP-REQ",
        pos_create_bkp_req,
        create_bkp_req_schema,
        openapi_path,
        openapi_doc,
    )

    neg_create_bkp_req = {"backup_id": "bkp-apifox-001", "skip_wal_sync": True}
    expect_invalid(
        "NEG-D10-CREATE-BACKUP-REQ-UNKNOWN-FIELD",
        neg_create_bkp_req,
        create_bkp_req_schema,
        openapi_path,
        "Unknown property 'skip_wal_sync' is forbidden",
        openapi_doc,
    )

    expect_valid(
        "POS-D10-VERIFY-BACKUP-REQ-EMPTY",
        {},
        verify_bkp_req_schema,
        openapi_path,
        openapi_doc,
    )

    pos_bkp_manifest = {
        "backup_id": "bkp-seed-001",
        "created_at_ms": 25800,
        "stream_epoch": "epoch-engine-01",
        "wal_checkpoint": "0000000000000001:0000000000000000",
        "sqlite_table_counts": {"product": 1, "device": 1, "resource": 2},
        "duckdb_timeseries_counts": {"latest_value": 1, "history_observation": 1},
        "files": [
            {
                "relative_path": "business_metadata.sqlite",
                "size_bytes": 40960,
                "sha256_hex": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            },
            {
                "relative_path": "timeseries_snapshot.json",
                "size_bytes": 1024,
                "sha256_hex": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            },
        ],
    }
    expect_valid(
        "POS-D10-BACKUP-MANIFEST",
        pos_bkp_manifest,
        bkp_manifest_schema,
        openapi_path,
        openapi_doc,
    )

    expect_valid(
        "POS-D10-BACKUP-LIST-RESP",
        {"items": [pos_bkp_manifest]},
        bkp_list_schema,
        openapi_path,
        openapi_doc,
    )

    pos_bkp_verify_report = {
        "backup_id": "bkp-seed-001",
        "verified": True,
        "checked_files": 3,
        "restored_sqlite_tables": 14,
        "restored_devices": 1,
        "restored_history_observations": 1,
        "restored_latest_values": 2,
        "wal_replayed_messages": 1,
        "issues": [],
    }
    expect_valid(
        "POS-D10-BACKUP-VERIFY-REPORT",
        pos_bkp_verify_report,
        bkp_verify_report_schema,
        openapi_path,
        openapi_doc,
    )

    neg_bkp_verify_report = copy.deepcopy(pos_bkp_verify_report)
    del neg_bkp_verify_report["wal_replayed_messages"]
    expect_invalid(
        "NEG-D10-BACKUP-VERIFY-REPORT-MISSING-WAL-REPLAYED",
        neg_bkp_verify_report,
        bkp_verify_report_schema,
        openapi_path,
        "Missing required property 'wal_replayed_messages'",
        openapi_doc,
    )

    print(
        f"OK: Verified 4 JSON Schemas, 1 OpenAPI 3.1 contract ({len(openapi_checks)} route/$ref checks), "
        f"{golden_checked} golden vector checks, and {len(results)} total positive/negative machine contract assertions."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
