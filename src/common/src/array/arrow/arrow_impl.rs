// Copyright 2024 RisingWave Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Converts between arrays and Apache Arrow arrays.
//!
//! This file acts as a template file for conversion code between
//! arrays and different version of Apache Arrow.
//!
//! The conversion logic will be implemented for the arrow version specified in the outer mod by
//! `super::arrow_xxx`, such as `super::arrow_array`.
//!
//! When we want to implement the conversion logic for an arrow version, we first
//! create a new mod file, and rename the corresponding arrow package name to `arrow_xxx`
//! using the `use` clause, and then declare a sub-mod and set its file path with attribute
//! `#[path = "./arrow_impl.rs"]` so that the code in this template file can be embedded to
//! the new mod file, and the conversion logic can be implemented for the corresponding arrow
//! version.
//!
//! Example can be seen in `arrow_default.rs`, which is also as followed:
//! ```ignore
//! use {arrow_array, arrow_buffer, arrow_cast, arrow_schema};
//!
//! #[allow(clippy::duplicate_mod)]
//! #[path = "./arrow_impl.rs"]
//! mod arrow_impl;
//! ```

use std::fmt::Write;
use std::sync::Arc;

use arrow_buffer::OffsetBuffer;
use chrono::{NaiveDateTime, NaiveTime};
use itertools::Itertools;

// This is important because we want to use the arrow version specified by the outer mod.
use super::{arrow_array, arrow_buffer, arrow_cast, arrow_schema};
// Other import should always use the absolute path.
use crate::array::*;
use crate::buffer::Bitmap;
use crate::types::*;
use crate::util::iter_util::ZipEqFast;

/// Defines how to convert RisingWave arrays to Arrow arrays.
pub trait ToArrow {
    /// Converts RisingWave `DataChunk` to Arrow `RecordBatch` with specified schema.
    ///
    /// This function will try to convert the array if the type is not same with the schema.
    fn to_record_batch(
        &self,
        schema: arrow_schema::SchemaRef,
        chunk: &DataChunk,
    ) -> Result<arrow_array::RecordBatch, ArrayError> {
        // compact the chunk if it's not compacted
        if !chunk.is_compacted() {
            let c = chunk.clone();
            return self.to_record_batch(schema, &c.compact());
        }

        // convert each column to arrow array
        let columns: Vec<_> = chunk
            .columns()
            .iter()
            .zip_eq_fast(schema.fields().iter())
            .map(|(column, field)| self.to_array(field.data_type(), column))
            .try_collect()?;

        // create record batch
        let opts =
            arrow_array::RecordBatchOptions::default().with_row_count(Some(chunk.capacity()));
        arrow_array::RecordBatch::try_new_with_options(schema, columns, &opts)
            .map_err(ArrayError::to_arrow)
    }

    /// Converts RisingWave array to Arrow array.
    fn to_array(
        &self,
        data_type: &arrow_schema::DataType,
        array: &ArrayImpl,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        let arrow_array = match array {
            ArrayImpl::Bool(array) => self.bool_to_arrow(array),
            ArrayImpl::Int16(array) => self.int16_to_arrow(array),
            ArrayImpl::Int32(array) => self.int32_to_arrow(array),
            ArrayImpl::Int64(array) => self.int64_to_arrow(array),
            ArrayImpl::Int256(array) => self.int256_to_arrow(array),
            ArrayImpl::Float32(array) => self.float32_to_arrow(array),
            ArrayImpl::Float64(array) => self.float64_to_arrow(array),
            ArrayImpl::Date(array) => self.date_to_arrow(array),
            ArrayImpl::Time(array) => self.time_to_arrow(array),
            ArrayImpl::Timestamp(array) => self.timestamp_to_arrow(array),
            ArrayImpl::Timestamptz(array) => self.timestamptz_to_arrow(array),
            ArrayImpl::Interval(array) => self.interval_to_arrow(array),
            ArrayImpl::Utf8(array) => self.utf8_to_arrow(array),
            ArrayImpl::Bytea(array) => self.bytea_to_arrow(array),
            ArrayImpl::Decimal(array) => self.decimal_to_arrow(data_type, array),
            ArrayImpl::Jsonb(array) => self.jsonb_to_arrow(array),
            ArrayImpl::Serial(array) => self.serial_to_arrow(array),
            ArrayImpl::List(array) => self.list_to_arrow(data_type, array),
            ArrayImpl::Struct(array) => self.struct_to_arrow(data_type, array),
        }?;
        if arrow_array.data_type() != data_type {
            arrow_cast::cast(&arrow_array, data_type).map_err(ArrayError::to_arrow)
        } else {
            Ok(arrow_array)
        }
    }

    #[inline]
    fn bool_to_arrow(&self, array: &BoolArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::BooleanArray::from(array)))
    }

    #[inline]
    fn int16_to_arrow(&self, array: &I16Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Int16Array::from(array)))
    }

    #[inline]
    fn int32_to_arrow(&self, array: &I32Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Int32Array::from(array)))
    }

    #[inline]
    fn int64_to_arrow(&self, array: &I64Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Int64Array::from(array)))
    }

    #[inline]
    fn float32_to_arrow(&self, array: &F32Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Float32Array::from(array)))
    }

    #[inline]
    fn float64_to_arrow(&self, array: &F64Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Float64Array::from(array)))
    }

    #[inline]
    fn utf8_to_arrow(&self, array: &Utf8Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::StringArray::from(array)))
    }

    #[inline]
    fn int256_to_arrow(&self, array: &Int256Array) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Decimal256Array::from(array)))
    }

    #[inline]
    fn date_to_arrow(&self, array: &DateArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Date32Array::from(array)))
    }

    #[inline]
    fn timestamp_to_arrow(
        &self,
        array: &TimestampArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::TimestampMicrosecondArray::from(
            array,
        )))
    }

    #[inline]
    fn timestamptz_to_arrow(
        &self,
        array: &TimestamptzArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(
            arrow_array::TimestampMicrosecondArray::from(array).with_timezone_utc(),
        ))
    }

    #[inline]
    fn time_to_arrow(&self, array: &TimeArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Time64MicrosecondArray::from(array)))
    }

    #[inline]
    fn interval_to_arrow(
        &self,
        array: &IntervalArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::IntervalMonthDayNanoArray::from(
            array,
        )))
    }

    #[inline]
    fn bytea_to_arrow(&self, array: &BytesArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::BinaryArray::from(array)))
    }

    // Decimal values are stored as ASCII text representation in a string array.
    #[inline]
    fn decimal_to_arrow(
        &self,
        _data_type: &arrow_schema::DataType,
        array: &DecimalArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::StringArray::from(array)))
    }

    // JSON values are stored as text representation in a string array.
    #[inline]
    fn jsonb_to_arrow(&self, array: &JsonbArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::StringArray::from(array)))
    }

    #[inline]
    fn serial_to_arrow(&self, array: &SerialArray) -> Result<arrow_array::ArrayRef, ArrayError> {
        Ok(Arc::new(arrow_array::Int64Array::from(array)))
    }

    #[inline]
    fn list_to_arrow(
        &self,
        data_type: &arrow_schema::DataType,
        array: &ListArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        let arrow_schema::DataType::List(field) = data_type else {
            return Err(ArrayError::to_arrow("Invalid list type"));
        };
        let values = self.to_array(field.data_type(), array.values())?;
        let offsets = OffsetBuffer::new(array.offsets().iter().map(|&o| o as i32).collect());
        let nulls = (!array.null_bitmap().all()).then(|| array.null_bitmap().into());
        Ok(Arc::new(arrow_array::ListArray::new(
            field.clone(),
            offsets,
            values,
            nulls,
        )))
    }

    #[inline]
    fn struct_to_arrow(
        &self,
        data_type: &arrow_schema::DataType,
        array: &StructArray,
    ) -> Result<arrow_array::ArrayRef, ArrayError> {
        let arrow_schema::DataType::Struct(fields) = data_type else {
            return Err(ArrayError::to_arrow("Invalid struct type"));
        };
        Ok(Arc::new(arrow_array::StructArray::new(
            fields.clone(),
            array
                .fields()
                .zip_eq_fast(fields)
                .map(|(arr, field)| self.to_array(field.data_type(), arr))
                .try_collect::<_, _, ArrayError>()?,
            Some(array.null_bitmap().into()),
        )))
    }

    /// Convert RisingWave data type to Arrow data type.
    ///
    /// This function returns a `Field` instead of `DataType` because some may be converted to
    /// extension types which require additional metadata in the field.
    fn to_arrow_field(
        &self,
        name: &str,
        value: &DataType,
    ) -> Result<arrow_schema::Field, ArrayError> {
        let data_type = match value {
            // using the inline function
            DataType::Boolean => self.bool_type_to_arrow(),
            DataType::Int16 => self.int16_type_to_arrow(),
            DataType::Int32 => self.int32_type_to_arrow(),
            DataType::Int64 => self.int64_type_to_arrow(),
            DataType::Int256 => self.int256_type_to_arrow(),
            DataType::Float32 => self.float32_type_to_arrow(),
            DataType::Float64 => self.float64_type_to_arrow(),
            DataType::Date => self.date_type_to_arrow(),
            DataType::Time => self.time_type_to_arrow(),
            DataType::Timestamp => self.timestamp_type_to_arrow(),
            DataType::Timestamptz => self.timestamptz_type_to_arrow(),
            DataType::Interval => self.interval_type_to_arrow(),
            DataType::Varchar => self.varchar_type_to_arrow(),
            DataType::Bytea => self.bytea_type_to_arrow(),
            DataType::Serial => self.serial_type_to_arrow(),
            DataType::Decimal => return Ok(self.decimal_type_to_arrow(name)),
            DataType::Jsonb => return Ok(self.jsonb_type_to_arrow(name)),
            DataType::Struct(fields) => self.struct_type_to_arrow(fields)?,
            DataType::List(datatype) => self.list_type_to_arrow(datatype)?,
        };
        Ok(arrow_schema::Field::new(name, data_type, true))
    }

    #[inline]
    fn bool_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Boolean
    }

    #[inline]
    fn int16_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Int16
    }

    #[inline]
    fn int32_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Int32
    }

    #[inline]
    fn int64_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Int64
    }

    #[inline]
    fn int256_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Decimal256(arrow_schema::DECIMAL256_MAX_PRECISION, 0)
    }

    #[inline]
    fn float32_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Float32
    }

    #[inline]
    fn float64_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Float64
    }

    #[inline]
    fn date_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Date32
    }

    #[inline]
    fn time_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Time64(arrow_schema::TimeUnit::Microsecond)
    }

    #[inline]
    fn timestamp_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None)
    }

    #[inline]
    fn timestamptz_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Timestamp(
            arrow_schema::TimeUnit::Microsecond,
            Some("+00:00".into()),
        )
    }

    #[inline]
    fn interval_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano)
    }

    #[inline]
    fn varchar_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Utf8
    }

    #[inline]
    fn jsonb_type_to_arrow(&self, name: &str) -> arrow_schema::Field {
        arrow_schema::Field::new(name, arrow_schema::DataType::Utf8, true)
            .with_metadata([("ARROW:extension:name".into(), "arrowudf.json".into())].into())
    }

    #[inline]
    fn bytea_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Binary
    }

    #[inline]
    fn decimal_type_to_arrow(&self, name: &str) -> arrow_schema::Field {
        arrow_schema::Field::new(name, arrow_schema::DataType::Utf8, true)
            .with_metadata([("ARROW:extension:name".into(), "arrowudf.decimal".into())].into())
    }

    #[inline]
    fn serial_type_to_arrow(&self) -> arrow_schema::DataType {
        arrow_schema::DataType::Int64
    }

    #[inline]
    fn list_type_to_arrow(
        &self,
        elem_type: &DataType,
    ) -> Result<arrow_schema::DataType, ArrayError> {
        Ok(arrow_schema::DataType::List(Arc::new(
            self.to_arrow_field("item", elem_type)?,
        )))
    }

    #[inline]
    fn struct_type_to_arrow(
        &self,
        fields: &StructType,
    ) -> Result<arrow_schema::DataType, ArrayError> {
        Ok(arrow_schema::DataType::Struct(
            fields
                .iter()
                .map(|(name, ty)| self.to_arrow_field(name, ty))
                .try_collect::<_, _, ArrayError>()?,
        ))
    }
}

/// Defines how to convert Arrow arrays to RisingWave arrays.
#[allow(clippy::wrong_self_convention)]
pub trait FromArrow {
    /// Converts Arrow `RecordBatch` to RisingWave `DataChunk`.
    fn from_record_batch(&self, batch: &arrow_array::RecordBatch) -> Result<DataChunk, ArrayError> {
        let mut columns = Vec::with_capacity(batch.num_columns());
        for (array, field) in batch.columns().iter().zip_eq_fast(batch.schema().fields()) {
            let column = Arc::new(self.from_array(field, array)?);
            columns.push(column);
        }
        Ok(DataChunk::new(columns, batch.num_rows()))
    }

    /// Converts Arrow `Fields` to RisingWave `StructType`.
    fn from_fields(&self, fields: &arrow_schema::Fields) -> Result<StructType, ArrayError> {
        Ok(StructType::new(
            fields
                .iter()
                .map(|f| Ok((f.name().clone(), self.from_field(f)?)))
                .try_collect::<_, _, ArrayError>()?,
        ))
    }

    /// Converts Arrow `Field` to RisingWave `DataType`.
    fn from_field(&self, field: &arrow_schema::Field) -> Result<DataType, ArrayError> {
        use arrow_schema::DataType::*;
        use arrow_schema::IntervalUnit::*;
        use arrow_schema::TimeUnit::*;

        // extension type
        if let Some(type_name) = field.metadata().get("ARROW:extension:name") {
            return self.from_extension_type(type_name, field.data_type());
        }

        Ok(match field.data_type() {
            Boolean => DataType::Boolean,
            Int16 => DataType::Int16,
            Int32 => DataType::Int32,
            Int64 => DataType::Int64,
            Float32 => DataType::Float32,
            Float64 => DataType::Float64,
            Decimal128(_, _) => DataType::Decimal,
            Decimal256(_, _) => DataType::Int256,
            Date32 => DataType::Date,
            Time64(Microsecond) => DataType::Time,
            Timestamp(Microsecond, None) => DataType::Timestamp,
            Timestamp(Microsecond, Some(_)) => DataType::Timestamptz,
            Interval(MonthDayNano) => DataType::Interval,
            Utf8 => DataType::Varchar,
            Binary => DataType::Bytea,
            LargeUtf8 => self.from_large_utf8()?,
            LargeBinary => self.from_large_binary()?,
            List(field) => DataType::List(Box::new(self.from_field(field)?)),
            Struct(fields) => DataType::Struct(self.from_fields(fields)?),
            t => {
                return Err(ArrayError::from_arrow(format!(
                    "unsupported arrow data type: {t:?}"
                )))
            }
        })
    }

    /// Converts Arrow `LargeUtf8` type to RisingWave data type.
    fn from_large_utf8(&self) -> Result<DataType, ArrayError> {
        Ok(DataType::Varchar)
    }

    /// Converts Arrow `LargeBinary` type to RisingWave data type.
    fn from_large_binary(&self) -> Result<DataType, ArrayError> {
        Ok(DataType::Bytea)
    }

    /// Converts Arrow extension type to RisingWave `DataType`.
    fn from_extension_type(
        &self,
        type_name: &str,
        physical_type: &arrow_schema::DataType,
    ) -> Result<DataType, ArrayError> {
        match (type_name, physical_type) {
            ("arrowudf.decimal", arrow_schema::DataType::Utf8) => Ok(DataType::Decimal),
            ("arrowudf.json", arrow_schema::DataType::Utf8) => Ok(DataType::Jsonb),
            _ => Err(ArrayError::from_arrow(format!(
                "unsupported extension type: {type_name:?}"
            ))),
        }
    }

    /// Converts Arrow `Array` to RisingWave `ArrayImpl`.
    fn from_array(
        &self,
        field: &arrow_schema::Field,
        array: &arrow_array::ArrayRef,
    ) -> Result<ArrayImpl, ArrayError> {
        use arrow_schema::DataType::*;
        use arrow_schema::IntervalUnit::*;
        use arrow_schema::TimeUnit::*;

        // extension type
        if let Some(type_name) = field.metadata().get("ARROW:extension:name") {
            return self.from_extension_array(type_name, array);
        }

        match array.data_type() {
            Boolean => self.from_bool_array(array.as_any().downcast_ref().unwrap()),
            Int16 => self.from_int16_array(array.as_any().downcast_ref().unwrap()),
            Int32 => self.from_int32_array(array.as_any().downcast_ref().unwrap()),
            Int64 => self.from_int64_array(array.as_any().downcast_ref().unwrap()),
            Decimal256(_, _) => self.from_int256_array(array.as_any().downcast_ref().unwrap()),
            Float32 => self.from_float32_array(array.as_any().downcast_ref().unwrap()),
            Float64 => self.from_float64_array(array.as_any().downcast_ref().unwrap()),
            Date32 => self.from_date32_array(array.as_any().downcast_ref().unwrap()),
            Time64(Microsecond) => self.from_time64us_array(array.as_any().downcast_ref().unwrap()),
            Timestamp(Microsecond, _) => {
                self.from_timestampus_array(array.as_any().downcast_ref().unwrap())
            }
            Interval(MonthDayNano) => {
                self.from_interval_array(array.as_any().downcast_ref().unwrap())
            }
            Utf8 => self.from_utf8_array(array.as_any().downcast_ref().unwrap()),
            Binary => self.from_binary_array(array.as_any().downcast_ref().unwrap()),
            LargeUtf8 => self.from_large_utf8_array(array.as_any().downcast_ref().unwrap()),
            LargeBinary => self.from_large_binary_array(array.as_any().downcast_ref().unwrap()),
            List(_) => self.from_list_array(array.as_any().downcast_ref().unwrap()),
            Struct(_) => self.from_struct_array(array.as_any().downcast_ref().unwrap()),
            t => Err(ArrayError::from_arrow(format!(
                "unsupported arrow data type: {t:?}",
            ))),
        }
    }

    /// Converts Arrow extension array to RisingWave `ArrayImpl`.
    fn from_extension_array(
        &self,
        type_name: &str,
        array: &arrow_array::ArrayRef,
    ) -> Result<ArrayImpl, ArrayError> {
        match type_name {
            "arrowudf.decimal" => {
                let array: &arrow_array::StringArray =
                    array.as_any().downcast_ref().ok_or_else(|| {
                        ArrayError::from_arrow(
                            "expected string array for `arrowudf.decimal`".to_string(),
                        )
                    })?;
                Ok(ArrayImpl::Decimal(array.try_into()?))
            }
            "arrowudf.json" => {
                let array: &arrow_array::StringArray =
                    array.as_any().downcast_ref().ok_or_else(|| {
                        ArrayError::from_arrow(
                            "expected string array for `arrowudf.json`".to_string(),
                        )
                    })?;
                Ok(ArrayImpl::Jsonb(array.try_into()?))
            }
            _ => Err(ArrayError::from_arrow(format!(
                "unsupported extension type: {type_name:?}"
            ))),
        }
    }

    fn from_bool_array(&self, array: &arrow_array::BooleanArray) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Bool(array.into()))
    }

    fn from_int16_array(&self, array: &arrow_array::Int16Array) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Int16(array.into()))
    }

    fn from_int32_array(&self, array: &arrow_array::Int32Array) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Int32(array.into()))
    }

    fn from_int64_array(&self, array: &arrow_array::Int64Array) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Int64(array.into()))
    }

    fn from_int256_array(
        &self,
        array: &arrow_array::Decimal256Array,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Int256(array.into()))
    }

    fn from_float32_array(
        &self,
        array: &arrow_array::Float32Array,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Float32(array.into()))
    }

    fn from_float64_array(
        &self,
        array: &arrow_array::Float64Array,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Float64(array.into()))
    }

    fn from_date32_array(&self, array: &arrow_array::Date32Array) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Date(array.into()))
    }

    fn from_time64us_array(
        &self,
        array: &arrow_array::Time64MicrosecondArray,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Time(array.into()))
    }

    fn from_timestampus_array(
        &self,
        array: &arrow_array::TimestampMicrosecondArray,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Timestamp(array.into()))
    }

    fn from_interval_array(
        &self,
        array: &arrow_array::IntervalMonthDayNanoArray,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Interval(array.into()))
    }

    fn from_utf8_array(&self, array: &arrow_array::StringArray) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Utf8(array.into()))
    }

    fn from_binary_array(&self, array: &arrow_array::BinaryArray) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Bytea(array.into()))
    }

    fn from_large_utf8_array(
        &self,
        array: &arrow_array::LargeStringArray,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Utf8(array.into()))
    }

    fn from_large_binary_array(
        &self,
        array: &arrow_array::LargeBinaryArray,
    ) -> Result<ArrayImpl, ArrayError> {
        Ok(ArrayImpl::Bytea(array.into()))
    }

    fn from_list_array(&self, array: &arrow_array::ListArray) -> Result<ArrayImpl, ArrayError> {
        use arrow_array::Array;
        let arrow_schema::DataType::List(field) = array.data_type() else {
            panic!("nested field types cannot be determined.");
        };
        Ok(ArrayImpl::List(ListArray {
            value: Box::new(self.from_array(field, array.values())?),
            bitmap: match array.nulls() {
                Some(nulls) => nulls.iter().collect(),
                None => Bitmap::ones(array.len()),
            },
            offsets: array.offsets().iter().map(|o| *o as u32).collect(),
        }))
    }

    fn from_struct_array(&self, array: &arrow_array::StructArray) -> Result<ArrayImpl, ArrayError> {
        use arrow_array::Array;
        let arrow_schema::DataType::Struct(fields) = array.data_type() else {
            panic!("nested field types cannot be determined.");
        };
        Ok(ArrayImpl::Struct(StructArray::new(
            self.from_fields(fields)?,
            array
                .columns()
                .iter()
                .zip_eq_fast(fields)
                .map(|(array, field)| self.from_array(field, array).map(Arc::new))
                .try_collect()?,
            (0..array.len()).map(|i| array.is_valid(i)).collect(),
        )))
    }
}

impl From<&Bitmap> for arrow_buffer::NullBuffer {
    fn from(bitmap: &Bitmap) -> Self {
        bitmap.iter().collect()
    }
}

/// Implement bi-directional `From` between concrete array types.
macro_rules! converts {
    ($ArrayType:ty, $ArrowType:ty) => {
        impl From<&$ArrayType> for $ArrowType {
            fn from(array: &$ArrayType) -> Self {
                array.iter().collect()
            }
        }
        impl From<&$ArrowType> for $ArrayType {
            fn from(array: &$ArrowType) -> Self {
                array.iter().collect()
            }
        }
        impl From<&[$ArrowType]> for $ArrayType {
            fn from(arrays: &[$ArrowType]) -> Self {
                arrays.iter().flat_map(|a| a.iter()).collect()
            }
        }
    };
    // convert values using FromIntoArrow
    ($ArrayType:ty, $ArrowType:ty, @map) => {
        impl From<&$ArrayType> for $ArrowType {
            fn from(array: &$ArrayType) -> Self {
                array.iter().map(|o| o.map(|v| v.into_arrow())).collect()
            }
        }
        impl From<&$ArrowType> for $ArrayType {
            fn from(array: &$ArrowType) -> Self {
                array
                    .iter()
                    .map(|o| {
                        o.map(|v| {
                            <<$ArrayType as Array>::RefItem<'_> as FromIntoArrow>::from_arrow(v)
                        })
                    })
                    .collect()
            }
        }
        impl From<&[$ArrowType]> for $ArrayType {
            fn from(arrays: &[$ArrowType]) -> Self {
                arrays
                    .iter()
                    .flat_map(|a| a.iter())
                    .map(|o| {
                        o.map(|v| {
                            <<$ArrayType as Array>::RefItem<'_> as FromIntoArrow>::from_arrow(v)
                        })
                    })
                    .collect()
            }
        }
    };
}
converts!(BoolArray, arrow_array::BooleanArray);
converts!(I16Array, arrow_array::Int16Array);
converts!(I32Array, arrow_array::Int32Array);
converts!(I64Array, arrow_array::Int64Array);
converts!(F32Array, arrow_array::Float32Array, @map);
converts!(F64Array, arrow_array::Float64Array, @map);
converts!(BytesArray, arrow_array::BinaryArray);
converts!(BytesArray, arrow_array::LargeBinaryArray);
converts!(Utf8Array, arrow_array::StringArray);
converts!(Utf8Array, arrow_array::LargeStringArray);
converts!(DateArray, arrow_array::Date32Array, @map);
converts!(TimeArray, arrow_array::Time64MicrosecondArray, @map);
converts!(TimestampArray, arrow_array::TimestampMicrosecondArray, @map);
converts!(TimestamptzArray, arrow_array::TimestampMicrosecondArray, @map);
converts!(IntervalArray, arrow_array::IntervalMonthDayNanoArray, @map);
converts!(SerialArray, arrow_array::Int64Array, @map);

/// Converts RisingWave value from and into Arrow value.
pub trait FromIntoArrow {
    /// The corresponding element type in the Arrow array.
    type ArrowType;
    fn from_arrow(value: Self::ArrowType) -> Self;
    fn into_arrow(self) -> Self::ArrowType;
}

impl FromIntoArrow for Serial {
    type ArrowType = i64;

    fn from_arrow(value: Self::ArrowType) -> Self {
        value.into()
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.into()
    }
}

impl FromIntoArrow for F32 {
    type ArrowType = f32;

    fn from_arrow(value: Self::ArrowType) -> Self {
        value.into()
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.into()
    }
}

impl FromIntoArrow for F64 {
    type ArrowType = f64;

    fn from_arrow(value: Self::ArrowType) -> Self {
        value.into()
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.into()
    }
}

impl FromIntoArrow for Date {
    type ArrowType = i32;

    fn from_arrow(value: Self::ArrowType) -> Self {
        Date(arrow_array::types::Date32Type::to_naive_date(value))
    }

    fn into_arrow(self) -> Self::ArrowType {
        arrow_array::types::Date32Type::from_naive_date(self.0)
    }
}

impl FromIntoArrow for Time {
    type ArrowType = i64;

    fn from_arrow(value: Self::ArrowType) -> Self {
        Time(
            NaiveTime::from_num_seconds_from_midnight_opt(
                (value / 1_000_000) as _,
                (value % 1_000_000 * 1000) as _,
            )
            .unwrap(),
        )
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.0
            .signed_duration_since(NaiveTime::default())
            .num_microseconds()
            .unwrap()
    }
}

impl FromIntoArrow for Timestamp {
    type ArrowType = i64;

    fn from_arrow(value: Self::ArrowType) -> Self {
        Timestamp(
            NaiveDateTime::from_timestamp_opt(
                (value / 1_000_000) as _,
                (value % 1_000_000 * 1000) as _,
            )
            .unwrap(),
        )
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.0
            .signed_duration_since(NaiveDateTime::default())
            .num_microseconds()
            .unwrap()
    }
}

impl FromIntoArrow for Timestamptz {
    type ArrowType = i64;

    fn from_arrow(value: Self::ArrowType) -> Self {
        Timestamptz::from_micros(value)
    }

    fn into_arrow(self) -> Self::ArrowType {
        self.timestamp_micros()
    }
}

impl FromIntoArrow for Interval {
    type ArrowType = i128;

    fn from_arrow(value: Self::ArrowType) -> Self {
        // XXX: the arrow-rs decoding is incorrect
        // let (months, days, ns) = arrow_array::types::IntervalMonthDayNanoType::to_parts(value);
        let months = value as i32;
        let days = (value >> 32) as i32;
        let ns = (value >> 64) as i64;
        Interval::from_month_day_usec(months, days, ns / 1000)
    }

    fn into_arrow(self) -> Self::ArrowType {
        // XXX: the arrow-rs encoding is incorrect
        // arrow_array::types::IntervalMonthDayNanoType::make_value(
        //     self.months(),
        //     self.days(),
        //     // TODO: this may overflow and we need `try_into`
        //     self.usecs() * 1000,
        // )
        let m = self.months() as u128 & u32::MAX as u128;
        let d = (self.days() as u128 & u32::MAX as u128) << 32;
        let n = ((self.usecs() * 1000) as u128 & u64::MAX as u128) << 64;
        (m | d | n) as i128
    }
}

impl From<&DecimalArray> for arrow_array::LargeBinaryArray {
    fn from(array: &DecimalArray) -> Self {
        let mut builder =
            arrow_array::builder::LargeBinaryBuilder::with_capacity(array.len(), array.len() * 8);
        for value in array.iter() {
            builder.append_option(value.map(|d| d.to_string()));
        }
        builder.finish()
    }
}

impl From<&DecimalArray> for arrow_array::StringArray {
    fn from(array: &DecimalArray) -> Self {
        let mut builder =
            arrow_array::builder::StringBuilder::with_capacity(array.len(), array.len() * 8);
        for value in array.iter() {
            builder.append_option(value.map(|d| d.to_string()));
        }
        builder.finish()
    }
}

// This arrow decimal type is used by iceberg source to read iceberg decimal into RW decimal.
impl TryFrom<&arrow_array::Decimal128Array> for DecimalArray {
    type Error = ArrayError;

    fn try_from(array: &arrow_array::Decimal128Array) -> Result<Self, Self::Error> {
        if array.scale() < 0 {
            bail!("support negative scale for arrow decimal")
        }
        let from_arrow = |value| {
            const NAN: i128 = i128::MIN + 1;
            let res = match value {
                NAN => Decimal::NaN,
                i128::MAX => Decimal::PositiveInf,
                i128::MIN => Decimal::NegativeInf,
                _ => Decimal::Normalized(
                    rust_decimal::Decimal::try_from_i128_with_scale(value, array.scale() as u32)
                        .map_err(ArrayError::internal)?,
                ),
            };
            Ok(res)
        };
        array
            .iter()
            .map(|o| o.map(from_arrow).transpose())
            .collect::<Result<Self, Self::Error>>()
    }
}

impl TryFrom<&arrow_array::LargeBinaryArray> for DecimalArray {
    type Error = ArrayError;

    fn try_from(array: &arrow_array::LargeBinaryArray) -> Result<Self, Self::Error> {
        array
            .iter()
            .map(|o| {
                o.map(|s| {
                    let s = std::str::from_utf8(s)
                        .map_err(|_| ArrayError::from_arrow(format!("invalid decimal: {s:?}")))?;
                    s.parse()
                        .map_err(|_| ArrayError::from_arrow(format!("invalid decimal: {s:?}")))
                })
                .transpose()
            })
            .try_collect()
    }
}

impl TryFrom<&arrow_array::StringArray> for DecimalArray {
    type Error = ArrayError;

    fn try_from(array: &arrow_array::StringArray) -> Result<Self, Self::Error> {
        array
            .iter()
            .map(|o| {
                o.map(|s| {
                    s.parse()
                        .map_err(|_| ArrayError::from_arrow(format!("invalid decimal: {s:?}")))
                })
                .transpose()
            })
            .try_collect()
    }
}

impl From<&JsonbArray> for arrow_array::StringArray {
    fn from(array: &JsonbArray) -> Self {
        let mut builder =
            arrow_array::builder::StringBuilder::with_capacity(array.len(), array.len() * 16);
        for value in array.iter() {
            match value {
                Some(jsonb) => {
                    write!(&mut builder, "{}", jsonb).unwrap();
                    builder.append_value("");
                }
                None => builder.append_null(),
            }
        }
        builder.finish()
    }
}

impl TryFrom<&arrow_array::StringArray> for JsonbArray {
    type Error = ArrayError;

    fn try_from(array: &arrow_array::StringArray) -> Result<Self, Self::Error> {
        array
            .iter()
            .map(|o| {
                o.map(|s| {
                    s.parse()
                        .map_err(|_| ArrayError::from_arrow(format!("invalid json: {s}")))
                })
                .transpose()
            })
            .try_collect()
    }
}

impl From<&JsonbArray> for arrow_array::LargeStringArray {
    fn from(array: &JsonbArray) -> Self {
        let mut builder =
            arrow_array::builder::LargeStringBuilder::with_capacity(array.len(), array.len() * 16);
        for value in array.iter() {
            match value {
                Some(jsonb) => {
                    write!(&mut builder, "{}", jsonb).unwrap();
                    builder.append_value("");
                }
                None => builder.append_null(),
            }
        }
        builder.finish()
    }
}

impl TryFrom<&arrow_array::LargeStringArray> for JsonbArray {
    type Error = ArrayError;

    fn try_from(array: &arrow_array::LargeStringArray) -> Result<Self, Self::Error> {
        array
            .iter()
            .map(|o| {
                o.map(|s| {
                    s.parse()
                        .map_err(|_| ArrayError::from_arrow(format!("invalid json: {s}")))
                })
                .transpose()
            })
            .try_collect()
    }
}

impl From<arrow_buffer::i256> for Int256 {
    fn from(value: arrow_buffer::i256) -> Self {
        let buffer = value.to_be_bytes();
        Int256::from_be_bytes(buffer)
    }
}

impl<'a> From<Int256Ref<'a>> for arrow_buffer::i256 {
    fn from(val: Int256Ref<'a>) -> Self {
        let buffer = val.to_be_bytes();
        arrow_buffer::i256::from_be_bytes(buffer)
    }
}

impl From<&Int256Array> for arrow_array::Decimal256Array {
    fn from(array: &Int256Array) -> Self {
        array
            .iter()
            .map(|o| o.map(arrow_buffer::i256::from))
            .collect()
    }
}

impl From<&arrow_array::Decimal256Array> for Int256Array {
    fn from(array: &arrow_array::Decimal256Array) -> Self {
        let values = array.iter().map(|o| o.map(Int256::from)).collect_vec();

        values
            .iter()
            .map(|i| i.as_ref().map(|v| v.as_scalar_ref()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool() {
        let array = BoolArray::from_iter([None, Some(false), Some(true)]);
        let arrow = arrow_array::BooleanArray::from(&array);
        assert_eq!(BoolArray::from(&arrow), array);
    }

    #[test]
    fn i16() {
        let array = I16Array::from_iter([None, Some(-7), Some(25)]);
        let arrow = arrow_array::Int16Array::from(&array);
        assert_eq!(I16Array::from(&arrow), array);
    }

    #[test]
    fn i32() {
        let array = I32Array::from_iter([None, Some(-7), Some(25)]);
        let arrow = arrow_array::Int32Array::from(&array);
        assert_eq!(I32Array::from(&arrow), array);
    }

    #[test]
    fn i64() {
        let array = I64Array::from_iter([None, Some(-7), Some(25)]);
        let arrow = arrow_array::Int64Array::from(&array);
        assert_eq!(I64Array::from(&arrow), array);
    }

    #[test]
    fn f32() {
        let array = F32Array::from_iter([None, Some(-7.0), Some(25.0)]);
        let arrow = arrow_array::Float32Array::from(&array);
        assert_eq!(F32Array::from(&arrow), array);
    }

    #[test]
    fn f64() {
        let array = F64Array::from_iter([None, Some(-7.0), Some(25.0)]);
        let arrow = arrow_array::Float64Array::from(&array);
        assert_eq!(F64Array::from(&arrow), array);
    }

    #[test]
    fn date() {
        let array = DateArray::from_iter([
            None,
            Date::with_days(12345).ok(),
            Date::with_days(-12345).ok(),
        ]);
        let arrow = arrow_array::Date32Array::from(&array);
        assert_eq!(DateArray::from(&arrow), array);
    }

    #[test]
    fn time() {
        let array = TimeArray::from_iter([None, Time::with_micro(24 * 3600 * 1_000_000 - 1).ok()]);
        let arrow = arrow_array::Time64MicrosecondArray::from(&array);
        assert_eq!(TimeArray::from(&arrow), array);
    }

    #[test]
    fn timestamp() {
        let array =
            TimestampArray::from_iter([None, Timestamp::with_micros(123456789012345678).ok()]);
        let arrow = arrow_array::TimestampMicrosecondArray::from(&array);
        assert_eq!(TimestampArray::from(&arrow), array);
    }

    #[test]
    fn interval() {
        let array = IntervalArray::from_iter([
            None,
            Some(Interval::from_month_day_usec(
                1_000_000,
                1_000,
                1_000_000_000,
            )),
            Some(Interval::from_month_day_usec(
                -1_000_000,
                -1_000,
                -1_000_000_000,
            )),
        ]);
        let arrow = arrow_array::IntervalMonthDayNanoArray::from(&array);
        assert_eq!(IntervalArray::from(&arrow), array);
    }

    #[test]
    fn string() {
        let array = Utf8Array::from_iter([None, Some("array"), Some("arrow")]);
        let arrow = arrow_array::StringArray::from(&array);
        assert_eq!(Utf8Array::from(&arrow), array);
    }

    #[test]
    fn binary() {
        let array = BytesArray::from_iter([None, Some("array".as_bytes())]);
        let arrow = arrow_array::BinaryArray::from(&array);
        assert_eq!(BytesArray::from(&arrow), array);
    }

    #[test]
    fn decimal() {
        let array = DecimalArray::from_iter([
            None,
            Some(Decimal::NaN),
            Some(Decimal::PositiveInf),
            Some(Decimal::NegativeInf),
            Some(Decimal::Normalized("123.4".parse().unwrap())),
            Some(Decimal::Normalized("123.456".parse().unwrap())),
        ]);
        let arrow = arrow_array::LargeBinaryArray::from(&array);
        assert_eq!(DecimalArray::try_from(&arrow).unwrap(), array);

        let arrow = arrow_array::StringArray::from(&array);
        assert_eq!(DecimalArray::try_from(&arrow).unwrap(), array);
    }

    #[test]
    fn jsonb() {
        let array = JsonbArray::from_iter([
            None,
            Some("null".parse().unwrap()),
            Some("false".parse().unwrap()),
            Some("1".parse().unwrap()),
            Some("[1, 2, 3]".parse().unwrap()),
            Some(r#"{ "a": 1, "b": null }"#.parse().unwrap()),
        ]);
        let arrow = arrow_array::LargeStringArray::from(&array);
        assert_eq!(JsonbArray::try_from(&arrow).unwrap(), array);

        let arrow = arrow_array::StringArray::from(&array);
        assert_eq!(JsonbArray::try_from(&arrow).unwrap(), array);
    }

    #[test]
    fn int256() {
        let values = [
            None,
            Some(Int256::from(1)),
            Some(Int256::from(i64::MAX)),
            Some(Int256::from(i64::MAX) * Int256::from(i64::MAX)),
            Some(Int256::from(i64::MAX) * Int256::from(i64::MAX) * Int256::from(i64::MAX)),
            Some(
                Int256::from(i64::MAX)
                    * Int256::from(i64::MAX)
                    * Int256::from(i64::MAX)
                    * Int256::from(i64::MAX),
            ),
            Some(Int256::min_value()),
            Some(Int256::max_value()),
        ];

        let array =
            Int256Array::from_iter(values.iter().map(|r| r.as_ref().map(|x| x.as_scalar_ref())));
        let arrow = arrow_array::Decimal256Array::from(&array);
        assert_eq!(Int256Array::from(&arrow), array);
    }
}
