use crate::error::EngineError;

pub fn init_core_schema(conn: &duckdb::Connection) -> Result<(), EngineError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS product (
            product_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            product_key VARCHAR NOT NULL,
            name VARCHAR NOT NULL,
            created_at_ms BIGINT NOT NULL,
            UNIQUE (tenant_id, product_key)
        );

        CREATE TABLE IF NOT EXISTS model_revision (
            model_revision_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            product_id VARCHAR NOT NULL,
            model_version VARCHAR NOT NULL,
            status VARCHAR NOT NULL,
            model_hash VARCHAR NOT NULL,
            published_at_ms BIGINT,
            UNIQUE (tenant_id, product_id, model_version)
        );

        CREATE TABLE IF NOT EXISTS model_item (
            model_revision_id VARCHAR NOT NULL,
            identifier VARCHAR NOT NULL,
            name VARCHAR NOT NULL,
            value_type VARCHAR NOT NULL,
            unit_code VARCHAR,
            unit_symbol VARCHAR,
            min_value DOUBLE,
            max_value DOUBLE,
            required BOOLEAN NOT NULL,
            PRIMARY KEY (model_revision_id, identifier)
        );

        CREATE TABLE IF NOT EXISTS asset (
            asset_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            parent_asset_id VARCHAR,
            slug VARCHAR NOT NULL,
            name VARCHAR NOT NULL,
            asset_path VARCHAR NOT NULL,
            UNIQUE (tenant_id, asset_path)
        );

        CREATE TABLE IF NOT EXISTS device (
            device_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            device_key VARCHAR NOT NULL,
            device_name VARCHAR NOT NULL,
            product_id VARCHAR NOT NULL,
            model_revision_id VARCHAR NOT NULL,
            active_binding_revision_id VARCHAR,
            asset_id VARCHAR NOT NULL,
            canonical_device_uri VARCHAR NOT NULL,
            revision BIGINT NOT NULL,
            UNIQUE (tenant_id, device_key)
        );

        CREATE TABLE IF NOT EXISTS resource (
            resource_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            property VARCHAR NOT NULL,
            name VARCHAR NOT NULL,
            canonical_uri VARCHAR NOT NULL,
            UNIQUE (device_id, property)
        );

        CREATE TABLE IF NOT EXISTS resource_path (
            uri VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            resource_id VARCHAR NOT NULL,
            path_state VARCHAR NOT NULL,
            redirect_to_uri VARCHAR,
            redirect_until_ms BIGINT
        );

        CREATE TABLE IF NOT EXISTS source_gateway (
            tenant_id VARCHAR NOT NULL,
            source_id VARCHAR NOT NULL,
            status VARCHAR NOT NULL,
            connection_state VARCHAR NOT NULL,
            PRIMARY KEY (tenant_id, source_id)
        );

        CREATE TABLE IF NOT EXISTS source_device_grant (
            tenant_id VARCHAR NOT NULL,
            source_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            valid_from_ms BIGINT NOT NULL,
            valid_to_ms BIGINT,
            PRIMARY KEY (tenant_id, source_id, device_id)
        );

        CREATE TABLE IF NOT EXISTS binding_revision (
            binding_revision_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            model_revision_id VARCHAR NOT NULL,
            source_id VARCHAR NOT NULL,
            normalization_owner VARCHAR NOT NULL,
            server_scale_factor DOUBLE,
            status VARCHAR NOT NULL,
            config_hash VARCHAR NOT NULL,
            valid_from_ms BIGINT,
            valid_to_ms BIGINT
        );

        CREATE TABLE IF NOT EXISTS point_binding (
            binding_revision_id VARCHAR NOT NULL,
            property VARCHAR NOT NULL,
            source_kind VARCHAR NOT NULL,
            register_address VARCHAR NOT NULL,
            factor DOUBLE NOT NULL,
            unit_code VARCHAR,
            deadband DOUBLE,
            heartbeat_ms BIGINT,
            static_value_json VARCHAR,
            PRIMARY KEY (binding_revision_id, property)
        );

        CREATE TABLE IF NOT EXISTS asset_relation (
            relation_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            from_asset_id VARCHAR NOT NULL,
            relation_type VARCHAR NOT NULL,
            to_asset_id VARCHAR NOT NULL,
            valid_from_ms BIGINT NOT NULL,
            valid_to_ms BIGINT,
            source VARCHAR NOT NULL,
            revision BIGINT NOT NULL,
            UNIQUE (tenant_id, from_asset_id, relation_type, to_asset_id)
        );

        CREATE TABLE IF NOT EXISTS edge_adapter (
            adapter_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            protocol VARCHAR NOT NULL,
            supported_value_types_json VARCHAR NOT NULL,
            supported_transforms_json VARCHAR NOT NULL,
            max_batch_size BIGINT NOT NULL,
            supports_deadband BOOLEAN NOT NULL,
            updated_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS derived_metric (
            metric_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            name VARCHAR NOT NULL,
            product_id VARCHAR NOT NULL,
            model_revision_id VARCHAR NOT NULL,
            expression VARCHAR NOT NULL,
            input_properties_json VARCHAR NOT NULL,
            output_unit_code VARCHAR,
            quality_policy VARCHAR NOT NULL,
            created_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS latest_value (
            resource_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            property VARCHAR NOT NULL,
            model_revision_id VARCHAR NOT NULL,
            binding_revision_id VARCHAR NOT NULL,
            value_type VARCHAR NOT NULL,
            value_json VARCHAR NOT NULL,
            value_f64 DOUBLE,
            value_i64 BIGINT,
            value_bool BOOLEAN,
            value_text VARCHAR,
            value_sampled_at_ms BIGINT NOT NULL,
            received_at_ms BIGINT NOT NULL,
            source_time_origin VARCHAR NOT NULL,
            source_epoch VARCHAR NOT NULL,
            source_seq_u64 UBIGINT NOT NULL,
            resource_revision BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS point_quality_state (
            resource_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            property VARCHAR NOT NULL,
            quality_state VARCHAR NOT NULL,
            quality_reason VARCHAR,
            quality_observed_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS quality_event (
            event_key VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            source_id VARCHAR NOT NULL,
            source_epoch VARCHAR NOT NULL,
            event_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            property VARCHAR,
            observed_at_ms BIGINT NOT NULL,
            connection_state VARCHAR NOT NULL,
            quality_state VARCHAR,
            quality_reason VARCHAR,
            event_hash VARCHAR NOT NULL
        );

        CREATE TABLE IF NOT EXISTS history_observation (
            observation_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            resource_id VARCHAR NOT NULL,
            device_id VARCHAR NOT NULL,
            property VARCHAR NOT NULL,
            model_revision_id VARCHAR NOT NULL,
            binding_revision_id VARCHAR NOT NULL,
            source_at_ms BIGINT NOT NULL,
            received_at_ms BIGINT NOT NULL,
            source_time_origin VARCHAR NOT NULL,
            source_seq VARCHAR NOT NULL,
            value_type VARCHAR NOT NULL,
            value_json VARCHAR NOT NULL,
            value_f64 DOUBLE,
            unit_code VARCHAR,
            unit_symbol VARCHAR,
            quality VARCHAR NOT NULL,
            source_quality_code VARCHAR,
            replayed BOOLEAN NOT NULL,
            wal_position VARCHAR NOT NULL
        );

        CREATE TABLE IF NOT EXISTS last_persisted_value (
            resource_id VARCHAR PRIMARY KEY,
            value_f64 DOUBLE,
            value_json VARCHAR NOT NULL,
            quality_state VARCHAR NOT NULL DEFAULT 'unknown',
            persisted_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS applied_message (
            tenant_id VARCHAR NOT NULL,
            source_id VARCHAR NOT NULL,
            source_epoch VARCHAR NOT NULL,
            message_id VARCHAR NOT NULL,
            ingest_id VARCHAR NOT NULL,
            payload_hash VARCHAR NOT NULL,
            wal_position VARCHAR NOT NULL,
            history_status VARCHAR NOT NULL,
            observation_ids_json VARCHAR NOT NULL,
            PRIMARY KEY (tenant_id, source_id, source_epoch, message_id)
        );

        CREATE TABLE IF NOT EXISTS ingest_status (
            ingest_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            message_id VARCHAR NOT NULL,
            status VARCHAR NOT NULL,
            history_status VARCHAR NOT NULL,
            wal_position VARCHAR NOT NULL,
            updated_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS target_checkpoint (
            target_id VARCHAR PRIMARY KEY,
            contiguous_wal_position VARCHAR NOT NULL,
            updated_at_ms BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS storage_delivery_ledger (
            target_backend VARCHAR NOT NULL,
            wal_position VARCHAR NOT NULL,
            ingest_id VARCHAR NOT NULL,
            status VARCHAR NOT NULL,
            reason VARCHAR,
            operator_id VARCHAR,
            recorded_at_ms BIGINT NOT NULL,
            PRIMARY KEY (target_backend, wal_position)
        );

        CREATE TABLE IF NOT EXISTS delivery_gap (
            gap_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            resource_id VARCHAR NOT NULL,
            start_ms BIGINT NOT NULL,
            end_ms BIGINT NOT NULL,
            reason VARCHAR NOT NULL
        );

        CREATE TABLE IF NOT EXISTS audit_log (
            audit_id VARCHAR PRIMARY KEY,
            tenant_id VARCHAR NOT NULL,
            caller_service_id VARCHAR NOT NULL,
            operator_id VARCHAR,
            action VARCHAR NOT NULL,
            entity_id VARCHAR NOT NULL,
            occurred_at_ms BIGINT NOT NULL
        );
        "#,
    )?;
    Ok(())
}
