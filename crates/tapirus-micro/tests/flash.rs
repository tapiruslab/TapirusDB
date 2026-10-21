use tapirus_micro::capsule::{Capsule, CapsuleError, HEADER_LEN, decode_capsule};
use tapirus_micro::{Error, Flash, Store, dot_i8};

#[derive(Clone)]
struct RamFlash {
    bytes: [u8; 1024],
    fail_after: Option<usize>,
    operations: usize,
}
impl RamFlash {
    fn fresh() -> Self {
        Self {
            bytes: [0xff; 1024],
            fail_after: None,
            operations: 0,
        }
    }
    fn step(&mut self) -> Result<(), &'static str> {
        if self.fail_after == Some(self.operations) {
            return Err("power interrupted");
        }
        self.operations += 1;
        Ok(())
    }
}
impl Flash for RamFlash {
    type Error = &'static str;
    fn capacity(&self) -> usize {
        self.bytes.len()
    }
    fn erase_size(&self) -> usize {
        256
    }
    fn write_size(&self) -> usize {
        8
    }
    fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), Self::Error> {
        output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
        Ok(())
    }
    fn erase(&mut self, offset: usize, length: usize) -> Result<(), Self::Error> {
        for i in offset..offset + length {
            self.step()?;
            self.bytes[i] = 0xff;
        }
        Ok(())
    }
    fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error> {
        assert_eq!(offset % 8, 0);
        assert_eq!(bytes.len() % 8, 0);
        for (i, byte) in bytes.iter().enumerate() {
            self.step()?;
            assert_eq!(self.bytes[offset + i] & byte, *byte);
            self.bytes[offset + i] &= byte;
        }
        Ok(())
    }
    fn sync(&mut self) -> Result<(), Self::Error> {
        self.step()
    }
}
#[test]
fn record_updates_delete_reopen_and_independent_slots() {
    let mut store = Store::<_, 256>::new(RamFlash::fresh(), 2).unwrap();
    let mut out = [0u8; 256];
    assert_eq!(store.max_value_bytes(), 216);
    assert_eq!(store.get(0, &mut out).unwrap(), None);
    store.put(0, b"old").unwrap();
    store.put(1, b"independent").unwrap();
    store.put(0, b"new").unwrap();
    store.put(0, b"latest").unwrap();
    let mut store = Store::<_, 256>::new(store.into_inner(), 2).unwrap();
    assert_eq!(store.get(0, &mut out).unwrap(), Some(6));
    assert_eq!(&out[..6], b"latest");
    store.delete(0).unwrap();
    assert_eq!(store.get(0, &mut out).unwrap(), None);
    assert_eq!(store.get(1, &mut out).unwrap(), Some(11));
    store.put(0, b"").unwrap();
    assert_eq!(store.get(0, &mut out).unwrap(), Some(0));
    assert!(matches!(store.put(2, b"x"), Err(Error::Slot)));
    assert!(matches!(store.put(0, &[0; 217]), Err(Error::Capacity)));
}
#[test]
fn interrupted_program_and_sync_never_expose_partial_payload() {
    let mut store = Store::<_, 256>::new(RamFlash::fresh(), 2).unwrap();
    store.put(0, b"old").unwrap();
    let baseline = store.into_inner();
    for cut in 0..516 {
        let mut flash = baseline.clone();
        flash.operations = 0;
        flash.fail_after = Some(cut);
        let mut store = Store::<_, 256>::new(flash, 2).unwrap();
        let _ = store.put(0, b"new payload");
        let mut flash = store.into_inner();
        flash.fail_after = None;
        let mut reopened = Store::<_, 256>::new(flash, 2).unwrap();
        let mut out = [0u8; 216];
        let n = reopened.get(0, &mut out).unwrap().unwrap();
        assert!(
            &out[..n] == b"old" || &out[..n] == b"new payload",
            "cut {cut}"
        );
    }
}
#[test]
fn committed_corruption_fails_closed() {
    let mut store = Store::<_, 256>::new(RamFlash::fresh(), 2).unwrap();
    store.put(0, b"data").unwrap();
    let mut flash = store.into_inner();
    flash.bytes[32] ^= 1;
    let mut store = Store::<_, 256>::new(flash, 2).unwrap();
    assert!(matches!(store.get(0, &mut [0; 216]), Err(Error::Corrupted)));
    assert!(matches!(
        Store::<_, 128>::new(RamFlash::fresh(), 2),
        Err(Error::Geometry)
    ));
    assert_eq!(dot_i8(&[-128, 127], &[-128, 127]), Some(32513));
    assert_eq!(dot_i8(&[], &[]), None);
}

#[test]
fn committed_slot_exports_versioned_capsules_and_tombstones() {
    let mut store = Store::<_, 256>::new(RamFlash::fresh(), 2).unwrap();
    let mut frame = [0u8; 256];
    assert_eq!(store.export_capsule(0, [7; 16], &mut frame).unwrap(), None);
    store.put(0, b"temperature 24 C").unwrap();
    let length = store
        .export_capsule(0, [7; 16], &mut frame)
        .unwrap()
        .unwrap();
    let first = decode_capsule(&frame[..length]).unwrap();
    assert_eq!(first.device_id, [7; 16]);
    assert_eq!(first.sequence, 1);
    assert_eq!(first.slot, 0);
    assert_eq!(first.payload, b"temperature 24 C");
    assert!(!first.deleted);

    frame[HEADER_LEN] ^= 1;
    assert_eq!(
        decode_capsule(&frame[..length]),
        Err(CapsuleError::Checksum)
    );
    store.delete(0).unwrap();
    let length = store
        .export_capsule(0, [7; 16], &mut frame)
        .unwrap()
        .unwrap();
    let deleted = decode_capsule(&frame[..length]).unwrap();
    assert_eq!(deleted.sequence, 2);
    assert!(deleted.deleted);
    assert!(deleted.payload.is_empty());
}

#[test]
fn capsule_decoder_rejects_truncation_trailing_data_and_unsupported_version() {
    let mut frame = [0u8; 128];
    let len = Capsule {
        device_id: [1; 16],
        slot: 3,
        sequence: 9,
        deleted: false,
        payload: b"pump ready",
    }
    .encode_into(&mut frame)
    .unwrap();
    assert_eq!(
        decode_capsule(&frame[..len - 1]),
        Err(CapsuleError::Invalid)
    );
    assert_eq!(
        decode_capsule(&frame[..len + 1]),
        Err(CapsuleError::Invalid)
    );
    frame[8] = 2;
    assert_eq!(decode_capsule(&frame[..len]), Err(CapsuleError::Version));
    frame[8] = 1;
    assert_eq!(
        Capsule {
            device_id: [0; 16],
            slot: 0,
            sequence: 1,
            deleted: false,
            payload: b"x"
        }
        .encode_into(&mut frame),
        Err(CapsuleError::Invalid)
    );
}
