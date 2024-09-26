// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{Error, Write};

use crate::compiler::util::emit_doc_string;
use crate::compiler::wire::{emit_type, emit_type_check};
use crate::compiler::Compiler;
use crate::ir::CompIdent;

pub fn emit_table<W: Write>(
    compiler: &mut Compiler<'_>,
    out: &mut W,
    ident: &CompIdent,
) -> Result<(), Error> {
    let t = &compiler.library.table_declarations[ident];

    let name = t.name.type_name();

    // Write wire type

    emit_doc_string(out, t.attributes.doc_string())?;
    writeln!(
        out,
        r#"
        #[repr(C)]
        pub struct Wire{name}<'buf> {{
            table: ::fidl::WireTable<'buf>,
        }}
        "#,
    )?;

    // Write decode impl

    writeln!(
        out,
        r#"
        unsafe impl<'buf> ::fidl::Decode<'buf> for Wire{name}<'buf> {{
            fn decode(
                slot: ::fidl::Slot<'_, Self>,
                decoder: &mut ::fidl::decode::Decoder<'buf>,
            ) -> Result<(), ::fidl::decode::Error> {{
                ::fidl::munge!(let Self {{ table }} = slot);

                ::fidl::WireTable::decode_with(
                    table,
                    decoder,
                    |ordinal, mut slot, decoder| match ordinal {{
                        0 => unsafe {{ ::core::hint::unreachable_unchecked() }},
        "#,
    )?;

    for member in &t.members {
        let name = &member.name;
        let ord = member.ordinal;
        write!(
            out,
            r#"
            {ord} => {{
                ::fidl::WireEnvelope::decode_as::<
            "#,
        )?;
        emit_type(compiler, out, &member.ty)?;
        writeln!(out, ">(slot.as_mut(), decoder)?;")?;
        emit_type_check(
            out,
            |out| {
                write!(
                    out,
                    r#"
                    let {name} = unsafe {{
                        slot.deref_unchecked().deref_unchecked::<
                    "#
                )?;
                emit_type(compiler, out, &member.ty)?;
                writeln!(out, ">() }};")
            },
            &member.name,
            &member.ty.kind,
        )?;
        writeln!(
            out,
            r#"
                Ok(())
            }}
            "#,
        )?;
    }

    writeln!(
        out,
        r#"
                        _ => ::fidl::WireEnvelope::decode_unknown(
                            slot,
                            decoder,
                        ),
                    }},
                )
            }}
        }}
        "#
    )?;

    // Write inherent impls

    writeln!(out, "impl<'buf> Wire{name}<'buf> {{")?;

    for member in &t.members {
        let ord = member.ordinal;
        let name = &member.name;

        write!(out, "pub fn {name}(&self) -> Option<&")?;
        emit_type(compiler, out, &member.ty)?;
        writeln!(
            out,
            r#"
            > {{
                unsafe {{
                    Some(self.table.get({ord})?.deref_unchecked())
                }}
            }}
            "#,
        )?;

        write!(out, "pub fn {name}_mut(&mut self) -> Option<&mut ")?;
        emit_type(compiler, out, &member.ty)?;
        writeln!(
            out,
            r#"
            > {{
                unsafe {{
                    Some(self.table.get_mut({ord})?.deref_mut_unchecked())
                }}
            }}
            "#,
        )?;

        write!(out, "pub fn take_{name}(&mut self) -> Option<")?;
        emit_type(compiler, out, &member.ty)?;
        writeln!(
            out,
            r#"
            > {{
                unsafe {{
                    Some(self.table.get_mut({ord})?.take_unchecked())
                }}
            }}
            "#,
        )?;
    }

    writeln!(out, "}}")?;

    // Write debug impl

    if compiler.config.emit_debug_impls {
        writeln!(
            out,
            r#"
            impl ::core::fmt::Debug for Wire{name}<'_> {{
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> Result<(), ::core::fmt::Error> {{
                    f.debug_struct("{name}")
            "#,
        )?;

        for member in &t.members {
            let name = &member.name;
            writeln!(out, ".field(\"{name}\", &self.{name}())")?;
        }

        writeln!(
            out,
            r#"
                    .finish()
                }}
            }}
            "#,
        )?;
    }

    Ok(())
}
