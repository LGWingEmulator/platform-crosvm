// Copyright 2020 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/*
 * automatically generated by rust-bindgen
 * $ bindgen /usr/include/stdio.h \
 *       --no-layout-tests \
 *       --whitelist-function vsnprintf \
 *       -o vsnprintf.rs
 */

extern "C" {
    pub fn vsnprintf(
        __s: *mut ::std::os::raw::c_char,
        __maxlen: ::std::os::raw::c_ulong,
        __format: *const ::std::os::raw::c_char,
        __arg: *mut __va_list_tag,
    ) -> ::std::os::raw::c_int;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __va_list_tag {
    pub gp_offset: ::std::os::raw::c_uint,
    pub fp_offset: ::std::os::raw::c_uint,
    pub overflow_arg_area: *mut ::std::os::raw::c_void,
    pub reg_save_area: *mut ::std::os::raw::c_void,
}
