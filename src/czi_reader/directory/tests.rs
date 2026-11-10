use crate::error::Error;

use super::DimFields;

#[test]
fn an_unmodelled_dimension_with_a_nonzero_start_is_refused_by_name() {
    match DimFields::new().apply(b'Q', 7, 1, 1) {
        Err(Error::Unsupported(msg)) => {
            assert_eq!(msg, "CZI has an unmodelled dimension 'Q' with Start=7 (not supported)")
        }
        _ => panic!("an unmodelled dimension with Start=7 must be Unsupported"),
    }
}

#[test]
fn an_unmodelled_dimension_at_index_zero_is_accepted() {
    assert!(DimFields::new().apply(b'Q', 0, 1, 1).is_ok());
}

#[test]
fn an_entry_without_its_y_dimension_is_a_parse_error() {
    let mut fields = DimFields::new();
    fields.apply(b'X', 0, 4, 4).unwrap();
    match fields.spatial() {
        Err(Error::Parse(msg)) => {
            assert_eq!(msg, "CZI subblock is missing an X or Y dimension entry")
        }
        _ => panic!("an entry without Y must be a Parse error"),
    }
}

#[test]
fn an_entry_with_both_spatial_dimensions_yields_their_sizes() {
    let mut fields = DimFields::new();
    fields.apply(b'X', 0, 4, 4).unwrap();
    fields.apply(b'Y', 0, 6, 5).unwrap();
    assert_eq!(fields.spatial().unwrap(), (4, 6, 4, 5));
}
