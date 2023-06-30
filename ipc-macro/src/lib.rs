// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use syn::{Fields, FnArg, ItemFn, ItemStruct, Type};

use serde::Deserialize;
use serde_tokenstream::from_tokenstream;

fn generate_dual_struct(s: &ItemStruct) -> (TokenStream2, TokenStream2) {
    let struct_ident = s.ident.clone();
    let dual_struct_ident = format_ident!("__ImplDual_{}", s.ident);

    let mut dual_struct = s.clone();
    let mut from_field_impls = Vec::new();
    let mut to_field_impls = Vec::new();

    dual_struct.ident = dual_struct_ident.clone();

    let Fields::Named(fields) = &mut dual_struct.fields else {
        panic!("This macro only works on structs with named fields")
    };

    for field in fields.named.iter_mut() {
        let id = field.ident.clone().unwrap();
        let ty = field.ty.clone();
        let origin_type = field.ty.clone();
        let map_type_stream = quote! { <#origin_type as ipc::packet::codec::FromPacket>::Dual };
        let map_type: Type = syn::parse2(map_type_stream).unwrap();

        from_field_impls.push(quote! {
            #id: <#ty>::decode_from_dual(value.#id, fds)
        });

        to_field_impls.push(quote! {
            #id: self.#id.encode_to_dual(fds)
        });

        field.ty = map_type;
    }

    let impls = quote! {
        impl FromPacket for #struct_ident {
            type Dual = #dual_struct_ident;
            fn decode_from_dual(value: Self::Dual, fds: &[std::os::fd::RawFd]) -> Self {
                Self {
                    #(#from_field_impls),*
                }
            }
            fn encode_to_dual(self, fds: &mut Vec<std::os::fd::RawFd>) -> Self::Dual {
                #dual_struct_ident {
                    #(#to_field_impls),*
                }
            }
        }
    };

    let added_macros_dual = quote! {
        #[allow(non_camel_case_types)]
        #[derive(Serialize, Deserialize)]
        #dual_struct
    };

    (added_macros_dual, impls)
}

#[proc_macro_derive(FromPacket)]
pub fn derive_from_packet(tokens: TokenStream) -> TokenStream {
    let item: syn::ItemStruct = syn::parse(tokens).unwrap();
    let (dual, imp) = generate_dual_struct(&item);
    (quote! {
        #dual
        #imp
    })
    .into()
}

#[derive(Deserialize)]
struct Meta {
    method: String,
}

#[proc_macro_attribute]
pub fn ipc_method(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let metadata: Meta = from_tokenstream(&attr.into()).unwrap();
    _method(metadata.method, item)
}

fn _method(method: String, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let function: ItemFn = syn::parse(item).unwrap();
    let ident = function.sig.ident;
    let block = function.block.to_token_stream();
    let args = function.sig.inputs;

    if args.len() != 3 {
        panic!();
    }

    let context = {
        match &args[0] {
            FnArg::Typed(thing) => thing.clone().ty.into_token_stream(),
            _ => panic!(),
        }
    };

    let local = {
        match &args[1] {
            FnArg::Typed(thing) => match *thing.ty.clone() {
                Type::Reference(reference) => reference.elem.into_token_stream(),
                _ => panic!(),
            },
            _ => panic!(),
        }
    };

    let request = {
        match &args[2] {
            FnArg::Typed(thing) => thing.clone().ty.into_token_stream(),
            _ => panic!(),
        }
    };

    let output = {
        match &function.sig.output {
            syn::ReturnType::Type(_, ty) => ty.into_token_stream(),
            _ => panic!(),
        }
    };

    let struct_ident = format_ident!("__ImplStruct{ident}");
    let struct_stream = quote! {
        #[allow(non_camel_case_types)]
        struct #struct_ident;
    };
    let const_stream = quote! {
        #[allow(non_upper_case_globals)]
        const #ident: #struct_ident = #struct_ident{};
    };

    let impl_handle = quote! {
        async fn apply(
            &self,
            context: #context,
            local_context: &mut #local,
            packet: ipc::packet::codec::json::JsonPacket,
        ) -> ipc::packet::TypedPacket<ipc::proto::Response>
        {
            async fn inner(context: #context, local_context: &mut #local, request: #request) -> #output
            {
                #block
            }

            let request  = <#request>::from_packet(
                packet, |dual| serde_json::from_value(dual.clone()).unwrap());

            let response = inner(
                context,
                local_context,
                request
            ).await;
            match response {
                Ok(response) => {
                    response.to_packet(|dual| {
                        ipc::proto::Response {
                            errno: 0,
                            value: serde_json::to_value(&dual).unwrap()
                        }
                    })
                },
                Err(eres) => {
                    ipc::packet::TypedPacket {
                        fds: Vec::new(),
                        data: ipc::proto::Response {
                            errno: eres.errno,
                            value: serde_json::to_value(&eres.value).unwrap()
                        }
                    }
                }
            }
        }
    };

    let token_stream = quote! {
        #[async_trait::async_trait]
        impl ipc::service::Method<
            <#context as ipc::util::ExtractInner>::Inner,
            <#local as ipc::util::ExtractInner>::Inner
        >
        for #struct_ident
        {
            fn identifier(&self) -> &'static str { #method }
            #impl_handle
        }
    };

    let client_fn_name = format_ident!("do_{ident}");
    let client_stream = quote! {
        pub fn #client_fn_name(
            stream: &mut UnixStream,
            request: #request
        ) -> Result<
                #output,
                ipc::transport::ChannelError<ipc::proto::IpcError>
            >
        {
            use ipc::transport::PacketTransport;
            use ipc::packet::codec::json::JsonPacket;
            use ipc::proto::IpcError;

            let packet = request.to_packet_failable(|dual| {
                let value = serde_json::to_value(dual)?;
                let req = ipc::proto::Request { method: #method.to_string(), value };
                serde_json::to_vec(&req)
            }).map_err(IpcError::Serde)?;

            stream.send_packet(&packet).map_err(|e| e.map(IpcError::Io))?;

            let packet = stream.recv_packet().map_err(|e| e.map(IpcError::Io))?;
            let json_packet = JsonPacket::new(packet).map_err(IpcError::Serde)?;
            let response: ipc::packet::TypedPacket<ipc::proto::Response> =
                json_packet
                    .map_failable(|value| serde_json::from_value(value.clone()))
                    .map_err(IpcError::Serde)?;

            if response.data.errno == 0 {
                let t = <<#output as ipc::util::ExtractResult>::Ok>::from_packet_failable(response, |inner| {
                    serde_json::from_value(inner.value.clone())
                }).map_err(IpcError::Serde)?;

                Ok(Ok(t))
            } else {
                let err = response.data.to_err_typed()?;
                Ok(Err(err))
            }
        }
    };
    quote! {
        #struct_stream
        #const_stream
        #token_stream
        #client_stream
    }
    .into()
}
