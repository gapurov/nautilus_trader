// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

use pyo3::prelude::*;

use crate::generated::{UnusualWhalesChannelForm, UnusualWhalesOperationId, find_operation};

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl UnusualWhalesOperationId {
    #[getter]
    #[pyo3(name = "operation_id")]
    fn py_operation_id(&self) -> &str {
        self.operation_id()
    }

    #[getter]
    fn method(&self) -> &str {
        find_operation(self.operation_id()).map_or("", |operation| operation.method)
    }

    #[getter]
    fn path(&self) -> &str {
        find_operation(self.operation_id()).map_or("", |operation| operation.path)
    }

    #[getter]
    fn is_read_only(&self) -> bool {
        find_operation(self.operation_id()).is_some_and(|operation| {
            operation.classification == crate::generated::OperationClassification::Read
        })
    }
}

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl UnusualWhalesChannelForm {
    #[getter]
    #[pyo3(name = "form")]
    fn py_form(&self) -> &str {
        self.form()
    }
}
