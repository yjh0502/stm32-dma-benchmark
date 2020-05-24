#![no_main]
#![no_std]

use panic_semihosting as _;

use core::fmt::{Display, Write};
use core::sync::atomic::{self, Ordering};

use cortex_m::peripheral::DWT;
use cortex_m_semihosting::hprintln;

use cortex_m_rt::entry;
use stm32f1xx_hal::{dma::dma1, pac, prelude::*};

fn clear<T: Default>(buf: &mut [T]) {
    for v in buf {
        *v = Default::default();
    }
}

fn set<T: From<u8>>(buf: &mut [T]) {
    for (i, v) in buf.iter_mut().enumerate() {
        *v = T::from(i as u8);
    }
}

fn validate<T: From<u8> + PartialEq + Display>(buf: &mut [T]) -> bool {
    for (i, v) in buf.iter().enumerate() {
        let expected = T::from(i as u8);
        if *v != expected {
            // panic!("idx={}, value={}, expected={}", i, &v, expected);
            return false;
        }
    }
    true
}

fn dma_set_addr<T1, T2>(channel: &mut dma1::C1, src: &[T1], dst: &mut [T2]) {
    assert_eq!(src.len(), dst.len());

    let mut src_size = core::mem::size_of::<T1>();
    let mut dst_size = core::mem::size_of::<T2>();

    let mut len = src.len();

    if src_size == dst_size && (src_size == 1 || src_size == 2) {
        while src_size < 4 {
            if len % 2 == 0 {
                src_size *= 2;
                dst_size *= 2;
                len = len / 2;
            }
        }
    }

    channel.set_peripheral_address(src.as_ptr() as u32, true);
    channel.set_memory_address(dst.as_ptr() as u32, true);

    channel.set_transfer_length(len);

    {
        atomic::compiler_fence(Ordering::Release);
        channel.ch().cr.modify(|_, w| {
            w.mem2mem().set_bit().circ().clear_bit().dir().clear_bit();

            match src_size {
                1 => w.psize().bits8(),
                2 => w.psize().bits16(),
                4 => w.psize().bits32(),
                sz => panic!("unsupported size: {}", sz),
            };

            match dst_size {
                1 => w.msize().bits8(),
                2 => w.msize().bits16(),
                4 => w.msize().bits32(),
                sz => panic!("unsupported size: {}", sz),
            };
            w
        });
    }
}

fn measure_cycles<F>(f: F) -> u32
where
    F: FnOnce(),
{
    let start_cyc = DWT::get_cycle_count();

    f();

    DWT::get_cycle_count() - start_cyc
}

macro_rules! bench {
    ($ch: ident, $buf_size: tt, $src_ty: ty, $dst_ty: ty) => {
        //
        {
            type SrcTy = $src_ty;
            type DstTy = $dst_ty;
            let mut src: [SrcTy; $buf_size] = [0; $buf_size];
            let mut dst: [DstTy; $buf_size] = [0; $buf_size];

            set(&mut src);
            clear(&mut dst);

            let dma_cycles = measure_cycles(|| {
                dma_set_addr(&mut $ch, &src, &mut dst);
                $ch.start();
                while $ch.in_progress() {}
                $ch.stop();
            });

            assert!(validate(&mut dst), "failed to validate dma result");
            clear(&mut dst);

            let cpu_cycles = measure_cycles(|| {
                for i in 0..src.len() {
                    dst[i] = (src[i] as DstTy).into();
                }
            });

            assert!(validate(&mut dst), "failed to validate CPU result");
            clear(&mut dst);

            let src_ty_size = core::mem::size_of::<SrcTy>() as u32;
            let read_size = $buf_size as u32 * src_ty_size;
            let dma_cycle_per_8byte = dma_cycles * 8 / read_size;
            let cpu_cycle_per_8byte = cpu_cycles * 8 / read_size;

            let mut msg = arrayvec::ArrayString::<[u8; 128]>::new();

            write!(
                &mut msg,
                "{},{},{},{},{},{},{}",
                $buf_size,
                stringify!($src_ty),
                stringify!($dst_ty),
                dma_cycles,
                dma_cycle_per_8byte,
                cpu_cycles,
                cpu_cycle_per_8byte,
            )
            .ok();
            hprintln!("{}", msg).ok();
        }
    };
}

macro_rules! bench_cases {
    ($ch: ident, $buf_size: tt, $(($src_ty: ty, $dst_ty: ty),)+) => {
        $(
            bench!($ch, $buf_size, $src_ty, $dst_ty);
        )+
    }
}

macro_rules! bench_run {
    ($ch: ident, $buf_size: tt) => {
        bench_cases!(
            $ch,
            $buf_size,
            (u8, u8),
            (u8, u16),
            (u8, u32),
            (u16, u8),
            (u16, u16),
            (u16, u32),
            (u32, u8),
            (u32, u16),
            (u32, u32),
        );
    };
}

#[entry]
fn main() -> ! {
    // Acquire peripherals
    let p = pac::Peripherals::take().unwrap();
    let mut flash = p.FLASH.constrain();
    let mut rcc = p.RCC.constrain();

    let clocks = rcc
        .cfgr
        .use_hse(8.mhz())
        .sysclk(72.mhz())
        .pclk1(36.mhz())
        .pclk2(72.mhz())
        .freeze(&mut flash.acr);

    hprintln!(
        "hclk={}, pclk1={} pclk2={} sysclk={}",
        clocks.hclk().0 / 1_000_000,
        clocks.pclk1().0 / 1_000_000,
        clocks.pclk2().0 / 1_000_000,
        clocks.sysclk().0 / 1_000_000,
    )
    .ok();

    hprintln!(
        "count,src_ty,dst_ty,dma_cycles,dma_cycles_per_8bytes,cpu_cycles,cpu_cycle_per_8bytes"
    )
    .ok();

    let mut channel = p.DMA1.split(&mut rcc.ahb).1;

    bench_run!(channel, 32);
    bench_run!(channel, 64);
    bench_run!(channel, 128);
    bench_run!(channel, 256);
    bench_run!(channel, 512);
    bench_run!(channel, 1024);
    bench_run!(channel, 2048);

    loop {}
}
