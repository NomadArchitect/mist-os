// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{Error, Write};

use crate::compiler::util::{
    emit_doc_string, int_type_natural_name, int_type_wire_name, snake_to_camel,
};
use crate::compiler::Compiler;
use crate::ir::CompIdent;

pub fn emit_enum<W: Write>(
    compiler: &mut Compiler<'_>,
    out: &mut W,
    ident: &CompIdent,
) -> Result<(), Error> {
    let e = &compiler.library.enum_declarations[ident];

    let name = &e.name.type_name();
    let natural_ty = int_type_natural_name(e.ty);
    let wire_ty = int_type_wire_name(e.ty);

    // Write natural type

    emit_doc_string(out, e.attributes.doc_string())?;
    writeln!(out, "#[derive(Clone, Copy)]")?;
    if compiler.config.emit_debug_impls {
        writeln!(out, "#[derive(Debug)]")?;
    }
    writeln!(
        out,
        r#"
        #[repr({natural_ty})]
        pub enum {name} {{
        "#,
    )?;

    for member in &e.members {
        let member_name = snake_to_camel(&member.name);
        let value = &member.constant.value;

        writeln!(out, "{member_name} = {value},")?;
    }

    if !e.is_strict {
        writeln!(out, "Unknown({natural_ty}),")?;
    }

    writeln!(out, "}}")?;

    // Write Encode impl

    writeln!(
        out,
        r#"
        impl ::fidl::Encode for {name} {{
            type Encoded<'buf> = Wire{name};

            fn encode(
                &mut self,
                encoder: &mut ::fidl::encode::Encoder,
                slot: ::fidl::Slot<'_, Self::Encoded<'_>>,
            ) -> Result<(), ::fidl::encode::Error> {{
                ::fidl::munge!(let Wire{name} {{ mut value }} = slot);
                *value = {wire_ty}::from(match *self {{
        "#,
    )?;

    for member in &e.members {
        let member_name = snake_to_camel(&member.name);
        let value = &member.constant.value;

        writeln!(out, "{name}::{member_name} => {value},")?;
    }

    if !e.is_strict {
        writeln!(out, "{name}::Unknown(value) => value,")?;
    }

    writeln!(
        out,
        r#"
                }});

                Ok(())
            }}
        }}
        "#,
    )?;

    // Write From impl

    write!(
        out,
        r#"
        impl ::core::convert::From<Wire{name}> for {name} {{
            fn from(wire: Wire{name}) -> Self {{
                match {natural_ty}::from(wire.value) {{
        "#,
    )?;

    for member in &e.members {
        let value = &member.constant.value;
        let member_name = snake_to_camel(&member.name);

        write!(out, "{value} => {name}::{member_name},")?;
    }

    if e.is_strict {
        writeln!(out, "_ => unsafe {{ ::core::hint::unreachable_unchecked() }},",)?;
    } else {
        writeln!(out, "value => {name}::Unknown(value),",)?;
    }

    writeln!(
        out,
        r#"
                }}
            }}
        }}
        "#,
    )?;

    // Write TakeFrom impl

    writeln!(
        out,
        r#"
        impl ::fidl::TakeFrom<Wire{name}> for {name} {{
            fn take_from(from: &mut Wire{name}) -> Self {{
                {name}::from(*from)
            }}
        }}
        "#,
    )?;

    Ok(())
}
