// Copyright 2019-2022 Parity Technologies (UK) Ltd.
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

#![deny(unused_crate_dependencies)]

#[cfg(test)]
use frame_metadata as _;
#[cfg(test)]
use prettyplease as _;
#[cfg(test)]
use scale_bits as _;

pub mod typegen;
pub mod utils;

pub use typegen::{
    error::TypegenError,
    settings::{
        derives::{Derives, DerivesRegistry},
        substitutes::TypeSubstitutes,
        TypeGeneratorSettings,
    },
    TypeGenerator,
};

#[cfg(test)]
mod tests;
