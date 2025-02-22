// Copyright 2024 RisingWave Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{HashMap, HashSet};

use fixedbitset::FixedBitSet;
use itertools::Itertools;
use risingwave_common::catalog::{
    ColumnCatalog, ConflictBehavior, CreateType, Field, Schema, StreamJobStatus, TableDesc,
    TableId, TableVersionId,
};
use risingwave_common::util::epoch::Epoch;
use risingwave_common::util::sort_util::ColumnOrder;
use risingwave_pb::catalog::table::{OptionalAssociatedSourceId, PbTableType, PbTableVersion};
use risingwave_pb::catalog::{PbCreateType, PbStreamJobStatus, PbTable};
use risingwave_pb::plan_common::column_desc::GeneratedOrDefaultColumn;
use risingwave_pb::plan_common::DefaultColumnDesc;

use super::{ColumnId, DatabaseId, FragmentId, OwnedByUserCatalog, SchemaId, SinkId};
use crate::error::{ErrorCode, RwError};
use crate::expr::ExprImpl;
use crate::optimizer::property::Cardinality;
use crate::user::UserId;

/// Includes full information about a table.
///
/// Currently, it can be either:
/// - a table or a source
/// - a materialized view
/// - an index
///
/// Use `self.table_type()` to determine the type of the table.
///
/// # Column ID & Column Index
///
/// [`ColumnId`](risingwave_common::catalog::ColumnId) (with type `i32`) is the unique identifier of
/// a column in a table. It is used to access storage.
///
/// Column index, or idx, (with type `usize`) is the relative position inside the `Vec` of columns.
///
/// A tip to avoid making mistakes is never do casting - i32 as usize or vice versa.
///
/// # Keys
///
/// All the keys are represented as column indices.
///
/// - **Primary Key** (pk): unique identifier of a row.
///
/// - **Order Key**: the primary key for storage, used to sort and access data.
///
///   For an MV, the columns in `ORDER BY` clause will be put at the beginning of the order key. And
/// the remaining columns in pk will follow behind.
///
///   If there's no `ORDER BY` clause, the order key will be the same as pk.
///
/// - **Distribution Key**: the columns used to partition the data. It must be a subset of the order
///   key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(Default))]
pub struct TableCatalog {
    pub id: TableId,

    pub associated_source_id: Option<TableId>, // TODO: use SourceId

    pub name: String,

    pub dependent_relations: Vec<TableId>,

    /// All columns in this table.
    pub columns: Vec<ColumnCatalog>,

    /// Key used as materialize's storage key prefix, including MV order columns and `stream_key`.
    pub pk: Vec<ColumnOrder>,

    /// `pk_indices` of the corresponding materialize operator's output.
    pub stream_key: Vec<usize>,

    /// Type of the table. Used to distinguish user-created tables, materialized views, index
    /// tables, and internal tables.
    pub table_type: TableType,

    /// Distribution key column indices.
    pub distribution_key: Vec<usize>,

    /// The append-only attribute is derived from `StreamMaterialize` and `StreamTableScan` relies
    /// on this to derive an append-only stream plan.
    pub append_only: bool,

    /// The cardinality of the table.
    pub cardinality: Cardinality,

    /// Owner of the table.
    pub owner: UserId,

    // TTL of the record in the table, to ensure the consistency with other tables in the streaming plan, it only applies to append-only tables.
    pub retention_seconds: Option<u32>,

    /// The fragment id of the `Materialize` operator for this table.
    pub fragment_id: FragmentId,

    /// The fragment id of the `DML` operator for this table.
    pub dml_fragment_id: Option<FragmentId>,

    /// An optional column index which is the vnode of each row computed by the table's consistent
    /// hash distribution.
    pub vnode_col_index: Option<usize>,

    /// An optional column index of row id. If the primary key is specified by users, this will be
    /// `None`.
    pub row_id_index: Option<usize>,

    /// The column indices which are stored in the state store's value with row-encoding.
    pub value_indices: Vec<usize>,

    /// The full `CREATE TABLE` or `CREATE MATERIALIZED VIEW` definition of the table.
    pub definition: String,

    /// The behavior of handling incoming pk conflict from source executor, we can overwrite or
    /// ignore conflict pk. For normal materialize executor and other executors, this field will be
    /// `No Check`.
    pub conflict_behavior: ConflictBehavior,

    pub version_column_index: Option<usize>,

    pub read_prefix_len_hint: usize,

    /// Per-table catalog version, used by schema change. `None` for internal tables and tests.
    pub version: Option<TableVersion>,

    /// The column indices which could receive watermarks.
    pub watermark_columns: FixedBitSet,

    /// Optional field specifies the distribution key indices in pk.
    /// See <https://github.com/risingwavelabs/risingwave/issues/8377> for more information.
    pub dist_key_in_pk: Vec<usize>,

    pub created_at_epoch: Option<Epoch>,

    pub initialized_at_epoch: Option<Epoch>,

    /// Indicate whether to use watermark cache for state table.
    pub cleaned_by_watermark: bool,

    /// Indicate whether to create table in background or foreground.
    pub create_type: CreateType,

    /// Indicate the stream job status, whether it is created or creating.
    /// If it is creating, we should hide it.
    pub stream_job_status: StreamJobStatus,

    /// description of table, set by `comment on`.
    pub description: Option<String>,

    /// Incoming sinks, used for sink into table
    pub incoming_sinks: Vec<SinkId>,

    pub created_at_cluster_version: Option<String>,

    pub initialized_at_cluster_version: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TableType {
    /// Tables created by `CREATE TABLE`.
    Table,
    /// Tables created by `CREATE MATERIALIZED VIEW`.
    MaterializedView,
    /// Tables serving as index for `TableType::Table` or `TableType::MaterializedView`.
    Index,
    /// Internal tables for executors.
    Internal,
}

#[cfg(test)]
impl Default for TableType {
    fn default() -> Self {
        Self::Table
    }
}

impl TableType {
    fn from_prost(prost: PbTableType) -> Self {
        match prost {
            PbTableType::Table => Self::Table,
            PbTableType::MaterializedView => Self::MaterializedView,
            PbTableType::Index => Self::Index,
            PbTableType::Internal => Self::Internal,
            PbTableType::Unspecified => unreachable!(),
        }
    }

    pub(crate) fn to_prost(self) -> PbTableType {
        match self {
            Self::Table => PbTableType::Table,
            Self::MaterializedView => PbTableType::MaterializedView,
            Self::Index => PbTableType::Index,
            Self::Internal => PbTableType::Internal,
        }
    }
}

/// The version of a table, used by schema change. See [`PbTableVersion`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TableVersion {
    pub version_id: TableVersionId,
    pub next_column_id: ColumnId,
}

impl TableVersion {
    /// Create an initial version for a table, with the given max column id.
    #[cfg(test)]
    pub fn new_initial_for_test(max_column_id: ColumnId) -> Self {
        use risingwave_common::catalog::INITIAL_TABLE_VERSION_ID;

        Self {
            version_id: INITIAL_TABLE_VERSION_ID,
            next_column_id: max_column_id.next(),
        }
    }

    pub fn from_prost(prost: PbTableVersion) -> Self {
        Self {
            version_id: prost.version,
            next_column_id: ColumnId::from(prost.next_column_id),
        }
    }

    pub fn to_prost(&self) -> PbTableVersion {
        PbTableVersion {
            version: self.version_id,
            next_column_id: self.next_column_id.into(),
        }
    }
}

impl TableCatalog {
    /// Get a reference to the table catalog's table id.
    pub fn id(&self) -> TableId {
        self.id
    }

    pub fn with_id(mut self, id: TableId) -> Self {
        self.id = id;
        self
    }

    pub fn with_cleaned_by_watermark(mut self, cleaned_by_watermark: bool) -> Self {
        self.cleaned_by_watermark = cleaned_by_watermark;
        self
    }

    pub fn conflict_behavior(&self) -> ConflictBehavior {
        self.conflict_behavior
    }

    pub fn table_type(&self) -> TableType {
        self.table_type
    }

    pub fn is_table(&self) -> bool {
        self.table_type == TableType::Table
    }

    pub fn is_internal_table(&self) -> bool {
        self.table_type == TableType::Internal
    }

    pub fn is_mview(&self) -> bool {
        self.table_type == TableType::MaterializedView
    }

    pub fn is_index(&self) -> bool {
        self.table_type == TableType::Index
    }

    /// Returns an error if `DROP` statements are used on the wrong type of table.
    #[must_use]
    pub fn bad_drop_error(&self) -> RwError {
        let msg = match self.table_type {
            TableType::MaterializedView => {
                "Use `DROP MATERIALIZED VIEW` to drop a materialized view."
            }
            TableType::Index => "Use `DROP INDEX` to drop an index.",
            TableType::Table => "Use `DROP TABLE` to drop a table.",
            TableType::Internal => "Internal tables cannot be dropped.",
        };

        ErrorCode::InvalidInputSyntax(msg.to_owned()).into()
    }

    /// Get the table catalog's associated source id.
    #[must_use]
    pub fn associated_source_id(&self) -> Option<TableId> {
        self.associated_source_id
    }

    pub fn has_associated_source(&self) -> bool {
        self.associated_source_id.is_some()
    }

    /// Get a reference to the table catalog's columns.
    pub fn columns(&self) -> &[ColumnCatalog] {
        &self.columns
    }

    /// Get a reference to the table catalog's pk desc.
    pub fn pk(&self) -> &[ColumnOrder] {
        self.pk.as_ref()
    }

    /// Get the column IDs of the primary key.
    pub fn pk_column_ids(&self) -> Vec<ColumnId> {
        self.pk
            .iter()
            .map(|x| self.columns[x.column_index].column_id())
            .collect()
    }

    /// Get a [`TableDesc`] of the table.
    pub fn table_desc(&self) -> TableDesc {
        use risingwave_common::catalog::TableOption;

        let table_options = TableOption::new(self.retention_seconds);

        TableDesc {
            table_id: self.id,
            pk: self.pk.clone(),
            stream_key: self.stream_key.clone(),
            columns: self.columns.iter().map(|c| c.column_desc.clone()).collect(),
            distribution_key: self.distribution_key.clone(),
            append_only: self.append_only,
            retention_seconds: table_options.retention_seconds,
            value_indices: self.value_indices.clone(),
            read_prefix_len_hint: self.read_prefix_len_hint,
            watermark_columns: self.watermark_columns.clone(),
            versioned: self.version.is_some(),
            vnode_col_index: self.vnode_col_index,
        }
    }

    /// Get a reference to the table catalog's name.
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn distribution_key(&self) -> &[usize] {
        self.distribution_key.as_ref()
    }

    pub fn to_internal_table_prost(&self) -> PbTable {
        use risingwave_common::catalog::{DatabaseId, SchemaId};
        self.to_prost(
            SchemaId::placeholder().schema_id,
            DatabaseId::placeholder().database_id,
        )
    }

    /// Returns the SQL statement that can be used to create this table.
    pub fn create_sql(&self) -> String {
        self.definition.clone()
    }

    /// Get a reference to the table catalog's version.
    pub fn version(&self) -> Option<&TableVersion> {
        self.version.as_ref()
    }

    /// Get the table's version id. Returns `None` if the table has no version field.
    pub fn version_id(&self) -> Option<TableVersionId> {
        self.version().map(|v| v.version_id)
    }

    pub fn to_prost(&self, schema_id: SchemaId, database_id: DatabaseId) -> PbTable {
        PbTable {
            id: self.id.table_id,
            schema_id,
            database_id,
            name: self.name.clone(),
            columns: self.columns().iter().map(|c| c.to_protobuf()).collect(),
            pk: self.pk.iter().map(|o| o.to_protobuf()).collect(),
            stream_key: self.stream_key.iter().map(|x| *x as _).collect(),
            dependent_relations: vec![],
            optional_associated_source_id: self
                .associated_source_id
                .map(|source_id| OptionalAssociatedSourceId::AssociatedSourceId(source_id.into())),
            table_type: self.table_type.to_prost() as i32,
            distribution_key: self
                .distribution_key
                .iter()
                .map(|k| *k as i32)
                .collect_vec(),
            append_only: self.append_only,
            owner: self.owner,
            fragment_id: self.fragment_id,
            dml_fragment_id: self.dml_fragment_id,
            vnode_col_index: self.vnode_col_index.map(|i| i as _),
            row_id_index: self.row_id_index.map(|i| i as _),
            value_indices: self.value_indices.iter().map(|x| *x as _).collect(),
            definition: self.definition.clone(),
            read_prefix_len_hint: self.read_prefix_len_hint as u32,
            version: self.version.as_ref().map(TableVersion::to_prost),
            watermark_indices: self.watermark_columns.ones().map(|x| x as _).collect_vec(),
            dist_key_in_pk: self.dist_key_in_pk.iter().map(|x| *x as _).collect(),
            handle_pk_conflict_behavior: self.conflict_behavior.to_protobuf().into(),
            version_column_index: self.version_column_index.map(|value| value as u32),
            cardinality: Some(self.cardinality.to_protobuf()),
            initialized_at_epoch: self.initialized_at_epoch.map(|epoch| epoch.0),
            created_at_epoch: self.created_at_epoch.map(|epoch| epoch.0),
            cleaned_by_watermark: self.cleaned_by_watermark,
            stream_job_status: self.stream_job_status.to_proto().into(),
            create_type: self.create_type.to_proto().into(),
            description: self.description.clone(),
            incoming_sinks: self.incoming_sinks.clone(),
            created_at_cluster_version: self.created_at_cluster_version.clone(),
            initialized_at_cluster_version: self.initialized_at_cluster_version.clone(),
            retention_seconds: self.retention_seconds,
        }
    }

    /// Get columns excluding hidden columns and generated golumns.
    pub fn columns_to_insert(&self) -> impl Iterator<Item = &ColumnCatalog> {
        self.columns
            .iter()
            .filter(|c| !c.is_hidden() && !c.is_generated())
    }

    pub fn generated_column_names(&self) -> impl Iterator<Item = &str> {
        self.columns
            .iter()
            .filter(|c| c.is_generated())
            .map(|c| c.name())
    }

    pub fn generated_col_idxes(&self) -> impl Iterator<Item = usize> + '_ {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_generated())
            .map(|(i, _)| i)
    }

    pub fn default_column_expr(&self, col_idx: usize) -> ExprImpl {
        if let Some(GeneratedOrDefaultColumn::DefaultColumn(DefaultColumnDesc { expr, .. })) = self
            .columns[col_idx]
            .column_desc
            .generated_or_default_column
            .as_ref()
        {
            ExprImpl::from_expr_proto(expr.as_ref().unwrap())
                .expect("expr in default columns corrupted")
        } else {
            ExprImpl::literal_null(self.columns[col_idx].data_type().clone())
        }
    }

    pub fn default_columns(&self) -> impl Iterator<Item = (usize, ExprImpl)> + '_ {
        self.columns.iter().enumerate().filter_map(|(i, c)| {
            if let Some(GeneratedOrDefaultColumn::DefaultColumn(DefaultColumnDesc {
                expr, ..
            })) = c.column_desc.generated_or_default_column.as_ref()
            {
                Some((
                    i,
                    ExprImpl::from_expr_proto(expr.as_ref().unwrap())
                        .expect("expr in default columns corrupted"),
                ))
            } else {
                None
            }
        })
    }

    pub fn has_generated_column(&self) -> bool {
        self.columns.iter().any(|c| c.is_generated())
    }

    pub fn column_schema(&self) -> Schema {
        Schema::new(
            self.columns
                .iter()
                .map(|c| Field::from(&c.column_desc))
                .collect(),
        )
    }
}

impl From<PbTable> for TableCatalog {
    fn from(tb: PbTable) -> Self {
        let id = tb.id;
        let tb_conflict_behavior = tb.handle_pk_conflict_behavior();
        let table_type = tb.get_table_type().unwrap();
        let stream_job_status = tb
            .get_stream_job_status()
            .unwrap_or(PbStreamJobStatus::Created);
        let create_type = tb.get_create_type().unwrap_or(PbCreateType::Foreground);
        let associated_source_id = tb.optional_associated_source_id.map(|id| match id {
            OptionalAssociatedSourceId::AssociatedSourceId(id) => id,
        });
        let name = tb.name.clone();
        let mut col_names = HashSet::new();
        let mut col_index: HashMap<i32, usize> = HashMap::new();

        let conflict_behavior = ConflictBehavior::from_protobuf(&tb_conflict_behavior);
        let version_column_index = tb.version_column_index.map(|value| value as usize);
        let columns: Vec<ColumnCatalog> = tb.columns.into_iter().map(ColumnCatalog::from).collect();
        for (idx, catalog) in columns.clone().into_iter().enumerate() {
            let col_name = catalog.name();
            if !col_names.insert(col_name.to_string()) {
                panic!("duplicated column name {} in table {} ", col_name, tb.name)
            }

            let col_id = catalog.column_desc.column_id.get_id();
            col_index.insert(col_id, idx);
        }

        let pk = tb.pk.iter().map(ColumnOrder::from_protobuf).collect();
        let mut watermark_columns = FixedBitSet::with_capacity(columns.len());
        for idx in &tb.watermark_indices {
            watermark_columns.insert(*idx as _);
        }

        Self {
            id: id.into(),
            associated_source_id: associated_source_id.map(Into::into),
            name,
            pk,
            columns,
            table_type: TableType::from_prost(table_type),
            distribution_key: tb
                .distribution_key
                .iter()
                .map(|k| *k as usize)
                .collect_vec(),
            stream_key: tb.stream_key.iter().map(|x| *x as _).collect(),
            append_only: tb.append_only,
            owner: tb.owner,
            fragment_id: tb.fragment_id,
            dml_fragment_id: tb.dml_fragment_id,
            vnode_col_index: tb.vnode_col_index.map(|x| x as usize),
            row_id_index: tb.row_id_index.map(|x| x as usize),
            value_indices: tb.value_indices.iter().map(|x| *x as _).collect(),
            definition: tb.definition,
            conflict_behavior,
            version_column_index,
            read_prefix_len_hint: tb.read_prefix_len_hint as usize,
            version: tb.version.map(TableVersion::from_prost),
            watermark_columns,
            dist_key_in_pk: tb.dist_key_in_pk.iter().map(|x| *x as _).collect(),
            cardinality: tb
                .cardinality
                .map(|c| Cardinality::from_protobuf(&c))
                .unwrap_or_else(Cardinality::unknown),
            created_at_epoch: tb.created_at_epoch.map(Epoch::from),
            initialized_at_epoch: tb.initialized_at_epoch.map(Epoch::from),
            cleaned_by_watermark: tb.cleaned_by_watermark,
            create_type: CreateType::from_proto(create_type),
            stream_job_status: StreamJobStatus::from_proto(stream_job_status),
            description: tb.description,
            incoming_sinks: tb.incoming_sinks.clone(),
            created_at_cluster_version: tb.created_at_cluster_version.clone(),
            initialized_at_cluster_version: tb.initialized_at_cluster_version.clone(),
            retention_seconds: tb.retention_seconds,
            dependent_relations: tb
                .dependent_relations
                .into_iter()
                .map(TableId::from)
                .collect_vec(),
        }
    }
}

impl From<&PbTable> for TableCatalog {
    fn from(tb: &PbTable) -> Self {
        tb.clone().into()
    }
}

impl OwnedByUserCatalog for TableCatalog {
    fn owner(&self) -> UserId {
        self.owner
    }
}

#[cfg(test)]
mod tests {

    use risingwave_common::catalog::{
        row_id_column_desc, ColumnCatalog, ColumnDesc, ColumnId, TableId,
    };
    use risingwave_common::test_prelude::*;
    use risingwave_common::types::*;
    use risingwave_common::util::sort_util::OrderType;
    use risingwave_pb::catalog::{PbStreamJobStatus, PbTable};
    use risingwave_pb::plan_common::{
        AdditionalColumn, ColumnDescVersion, PbColumnCatalog, PbColumnDesc,
    };

    use super::*;
    use crate::catalog::table_catalog::{TableCatalog, TableType};

    #[test]
    fn test_into_table_catalog() {
        let table: TableCatalog = PbTable {
            id: 0,
            schema_id: 0,
            database_id: 0,
            name: "test".to_string(),
            table_type: PbTableType::Table as i32,
            columns: vec![
                PbColumnCatalog {
                    column_desc: Some((&row_id_column_desc()).into()),
                    is_hidden: true,
                },
                PbColumnCatalog {
                    column_desc: Some(PbColumnDesc::new_struct(
                        "country",
                        1,
                        ".test.Country",
                        vec![
                            PbColumnDesc::new_atomic(DataType::Varchar.to_protobuf(), "address", 2),
                            PbColumnDesc::new_atomic(DataType::Varchar.to_protobuf(), "zipcode", 3),
                        ],
                    )),
                    is_hidden: false,
                },
            ],
            pk: vec![ColumnOrder::new(0, OrderType::ascending()).to_protobuf()],
            stream_key: vec![0],
            dependent_relations: vec![],
            distribution_key: vec![],
            optional_associated_source_id: OptionalAssociatedSourceId::AssociatedSourceId(233)
                .into(),
            append_only: false,
            owner: risingwave_common::catalog::DEFAULT_SUPER_USER_ID,
            retention_seconds: Some(300),
            fragment_id: 0,
            dml_fragment_id: None,
            initialized_at_epoch: None,
            value_indices: vec![0],
            definition: "".into(),
            read_prefix_len_hint: 0,
            vnode_col_index: None,
            row_id_index: None,
            version: Some(PbTableVersion {
                version: 0,
                next_column_id: 2,
            }),
            watermark_indices: vec![],
            handle_pk_conflict_behavior: 3,
            dist_key_in_pk: vec![],
            cardinality: None,
            created_at_epoch: None,
            cleaned_by_watermark: false,
            stream_job_status: PbStreamJobStatus::Created.into(),
            create_type: PbCreateType::Foreground.into(),
            description: Some("description".to_string()),
            incoming_sinks: vec![],
            created_at_cluster_version: None,
            initialized_at_cluster_version: None,
            version_column_index: None,
        }
        .into();

        assert_eq!(
            table,
            TableCatalog {
                id: TableId::new(0),
                associated_source_id: Some(TableId::new(233)),
                name: "test".to_string(),
                table_type: TableType::Table,
                columns: vec![
                    ColumnCatalog::row_id_column(),
                    ColumnCatalog {
                        column_desc: ColumnDesc {
                            data_type: DataType::new_struct(
                                vec![DataType::Varchar, DataType::Varchar],
                                vec!["address".to_string(), "zipcode".to_string()]
                            ),
                            column_id: ColumnId::new(1),
                            name: "country".to_string(),
                            field_descs: vec![
                                ColumnDesc::new_atomic(DataType::Varchar, "address", 2),
                                ColumnDesc::new_atomic(DataType::Varchar, "zipcode", 3),
                            ],
                            type_name: ".test.Country".to_string(),
                            description: None,
                            generated_or_default_column: None,
                            additional_column: AdditionalColumn { column_type: None },
                            version: ColumnDescVersion::Pr13707,
                        },
                        is_hidden: false
                    }
                ],
                stream_key: vec![0],
                pk: vec![ColumnOrder::new(0, OrderType::ascending())],
                distribution_key: vec![],
                append_only: false,
                owner: risingwave_common::catalog::DEFAULT_SUPER_USER_ID,
                retention_seconds: Some(300),
                fragment_id: 0,
                dml_fragment_id: None,
                vnode_col_index: None,
                row_id_index: None,
                value_indices: vec![0],
                definition: "".into(),
                conflict_behavior: ConflictBehavior::NoCheck,
                read_prefix_len_hint: 0,
                version: Some(TableVersion::new_initial_for_test(ColumnId::new(1))),
                watermark_columns: FixedBitSet::with_capacity(2),
                dist_key_in_pk: vec![],
                cardinality: Cardinality::unknown(),
                created_at_epoch: None,
                initialized_at_epoch: None,
                cleaned_by_watermark: false,
                stream_job_status: StreamJobStatus::Created,
                create_type: CreateType::Foreground,
                description: Some("description".to_string()),
                incoming_sinks: vec![],
                created_at_cluster_version: None,
                initialized_at_cluster_version: None,
                dependent_relations: vec![],
                version_column_index: None,
            }
        );
        assert_eq!(table, TableCatalog::from(table.to_prost(0, 0)));
    }
}
