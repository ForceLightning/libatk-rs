use crate::device::REPORT_ID;
use crate::types::{CommandId, EEPROMAddress, Error};
use strum::IntoEnumIterator;

// These are hardcoded for now as i don't know if there are other devices with different values.
// If that is the case then these can be set dynamically
/// Represents the offset from the start of the command to the first byte of the data field
static BASE_OFFSET: usize = 0x5;
static CMD_LEN: usize = 0x10;

/// A trait that allows to define new commands
pub trait CommandDescriptor {}

/*
The command layout is as follows:
┌────────────┬───────────────┬────────────────┬───────────────────┬──────────────┬──────────┐
│ Command ID │ Command Status│ EEPROM Address │ Data Valid Length │     Data     │ Checksum │
│   1 Byte   │    1 Byte     │   2 Bytes      │      1 Byte       │  10 Bytes    │  1 Byte  │
└────────────┴───────────────┴────────────────┴───────────────────┴──────────────┴──────────┘
*/

/// A generic command that stores the various fields (header, data payload, checksum)
/// of a command. The command is parameterized using a type which implements the
/// `CommandDescriptor` trait for command-specific size and layout definitions.
pub struct Command<T: CommandDescriptor> {
    command_id: CommandId,
    status: u8,
    eeprom_address: EEPROMAddress,
    eeprom_offset: u8,
    data_len: usize,
    data: Vec<u8>,
    checksum: u8,
    _cmd: std::marker::PhantomData<T>,
}

impl<T: CommandDescriptor> Clone for Command<T> {
    fn clone(&self) -> Self {
        Self {
            command_id: self.command_id,
            status: self.status,
            eeprom_address: self.eeprom_address,
            eeprom_offset: self.eeprom_offset,
            data_len: self.data_len,
            data: self.data.clone(),
            checksum: self.checksum,
            _cmd: std::marker::PhantomData,
        }
    }
}

impl<T: CommandDescriptor> std::fmt::Debug for Command<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:X?}", self.as_bytes())
    }
}

impl<T: CommandDescriptor> std::fmt::Display for Command<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ID: {:?}\nStatus: {}\nAddress: {:?} (0x{:X?}) + Offset (0x{:02X?}) = 0x{:X?}\nData Length: {}\nData: {:X?}\nChecksum: 0x{:02X?}",
            self.command_id,
            self.status,
            self.eeprom_address,
            self.eeprom_address as u16,
            self.eeprom_offset,
            self.eeprom_address as u16 + self.eeprom_offset as u16,
            self.data_len,
            &self.data[..self.data_len],
            self.checksum
        )
    }
}

impl<T: CommandDescriptor> Default for Command<T> {
    fn default() -> Self {
        Self {
            command_id: CommandId::Zero,
            status: 0,
            eeprom_address: EEPROMAddress::ReportRate,
            eeprom_offset: 0,
            data_len: 0,
            data: vec![0u8; CMD_LEN - BASE_OFFSET - 1],
            checksum: 0,
            _cmd: std::marker::PhantomData,
        }
    }
}

impl<T: CommandDescriptor> TryFrom<Vec<u8>> for Command<T> {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Command::try_from(value.as_ref())
    }
}

impl<T: CommandDescriptor> TryFrom<&[u8]> for Command<T> {
    type Error = Error;

    fn try_from(raw: &[u8]) -> Result<Self, Self::Error> {
        if raw.len() != CMD_LEN {
            return Err(Error::InvalidBufferLength {
                expected: CMD_LEN,
                actual: raw.len(),
            });
        }

        let command_id = raw[0x0].try_into()?;
        let status = raw[0x1];
        let raw_eeprom_address = u16::from_be_bytes([raw[0x2], raw[0x3]]);
        let eeprom_address: EEPROMAddress = EEPROMAddress::iter()
            .map(|x| x as u16)
            .filter(|&x| x <= raw_eeprom_address)
            .max()
            .map(|x| x.try_into())
            .expect("Invalid EEPROM address")?;
        let eeprom_offset = (raw_eeprom_address - (eeprom_address as u16)) as u8;
        let data_len = raw[0x4] as usize;
        let data = raw[BASE_OFFSET..BASE_OFFSET + data_len].to_vec();
        let checksum = raw[0xf];

        Ok(Self {
            command_id,
            status,
            eeprom_address,
            eeprom_offset,
            data_len,
            data,
            checksum,
            _cmd: std::marker::PhantomData,
        })
    }
}

impl<T: CommandDescriptor> Command<T> {
    /// Sets a slice of data into the command's data payload at a given offset.
    ///
    /// # Arguments
    ///
    /// * `data` - The slice of data to set.
    /// * `offset` - The offset in the data payload where this slice should be copied.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the data is set successfully.
    /// * `Err(Error::InvalidDataLength)` if the provided data length plus offset exceeds the valid data length.
    pub fn set_data(&mut self, data: &[u8], offset: usize) -> Result<(), Error> {
        if offset + data.len() > self.data_len() {
            return Err(Error::InvalidDataLength {
                offset,
                data_len: data.len(),
                allowed: self.data_len(),
            });
        }

        self.data[offset..offset + data.len()].copy_from_slice(data);
        self.set_checksum();
        Ok(())
    }

    /// Sets a single byte in the command's data payload.
    ///
    /// # Arguments
    ///
    /// * `value` - The byte value to be set.
    /// * `offset` - The index in the data payload where the byte should be written.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the value is successfully set.
    /// * `Err(Error::InvalidOffset)` if the offset is out of bounds.
    pub fn set_data_byte(&mut self, value: u8, offset: usize) -> Result<(), Error> {
        if offset > self.data_len() - 1 {
            return Err(Error::InvalidOffset(offset));
        }

        self.data[offset] = value;
        self.set_checksum();
        Ok(())
    }

    /// Sets a byte value at an even offset and also writes its complementary checksum byte.
    ///
    /// This method first writes the provided `value` at the given *even* `offset` and then writes
    /// the checksum (calculated as `0x55.warpping_sum(value)`) at the following byte.
    ///
    /// # Arguments
    ///
    /// * `value` - The byte value to be set.
    /// * `offset` - The starting offset (must be even) in the data payload.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the operation is successful.
    /// * `Err(&'static str)` if the offset is not aligned (not even) or out of bounds.
    pub fn set_data_byte_with_checksum(&mut self, value: u8, offset: usize) -> Result<(), Error> {
        if offset % 2 != 0 {
            return Err(Error::OffsetNotAligned(offset));
        }

        self.set_data_byte(value, offset)?;
        self.set_data_byte(0x55u8.wrapping_sub(value), offset + 1)?;

        Ok(())
    }

    pub fn id(&self) -> CommandId {
        self.command_id
    }

    /// Sets the command ID and updates the checksum afterward.
    pub fn set_id(&mut self, id: CommandId) {
        self.command_id = id;
        self.set_checksum();
    }

    pub fn status(&self) -> u8 {
        self.status
    }

    /// Sets the status byte and updates the checksum afterward.
    pub fn set_status(&mut self, status: u8) {
        self.status = status;
        self.set_checksum();
    }

    /// Returns the EEPROM address associated with the command.
    pub fn eeprom_address(&self) -> EEPROMAddress {
        self.eeprom_address
    }

    /// Sets the EEPROM address and updates the checksum.
    pub fn set_eeprom_address(&mut self, address: EEPROMAddress) {
        self.eeprom_address = address;
        self.set_checksum();
    }

    pub fn eeprom_offset(&self) -> u8 {
        self.eeprom_offset
    }

    pub fn set_eeprom_offset(&mut self, offset: u8) {
        self.eeprom_offset = offset;
        self.set_checksum();
    }

    /// Returns the valid length of the data payload.
    pub fn data_len(&self) -> usize {
        self.data_len
    }

    /// Sets the valid data length.
    ///
    /// # Panics
    ///
    /// Panics if the provided length exceeds the maximum available space computed via:
    /// `CMD_LEN - BASE_OFFSET`
    pub fn set_data_len(&mut self, len: usize) -> Result<(), Error> {
        if len > CMD_LEN - BASE_OFFSET {
            return Err(Error::DataTooLarge(len));
        }

        self.data_len = len;
        self.set_checksum();
        Ok(())
    }

    fn set_checksum(&mut self) {
        let sum: u8 = {
            let mut sum = REPORT_ID as u16;
            sum += self.command_id as u16;
            sum += self.status as u16;
            sum += (self.eeprom_address as u16 & 0xFF00) >> 8;
            sum += self.eeprom_address as u16 & 0x00FF;
            sum += self.eeprom_offset as u16;
            sum += self.data_len as u16;
            sum += self.data.iter().fold(0, |acc, &byte| acc + byte as u16);
            (sum & 0xff) as u8
        };
        // let checksum = 0x52u8.wrapping_sub(sum);
        let checksum = 0x55u8.wrapping_sub(sum);
        self.checksum = checksum;
    }

    /// Serializes the command into a vector of bytes.
    ///
    /// The serialization follows this order:
    /// 1. Command ID
    /// 2. Status
    /// 3. EEPROM address as big-endian bytes
    /// 4. Valid data length
    /// 5. Data payload
    /// 6. Checksum
    ///
    /// # Returns
    ///
    /// A vector containing the bytewise representation of the command.
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut raw = vec![self.command_id as u8, self.status];
        let true_eeprom_address = (self.eeprom_address as u16) + (self.eeprom_offset as u16);
        raw.extend_from_slice(&true_eeprom_address.to_be_bytes());
        raw.push(self.data_len as u8);
        raw.extend_from_slice(&self.data);
        // Pad the remaining bytes with zeroes
        let remaining = CMD_LEN - raw.len() - 1;
        raw.extend_from_slice(&vec![0u8; remaining]);
        raw.push(self.checksum);
        raw
    }

    /// Returns a reference to the data payload.
    ///
    /// # Returns
    ///
    /// A slice containing the data payload.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Executes the command on the specified device.
    ///
    /// # Returns
    ///
    /// Response of the command
    pub fn execute(&self, device: &crate::device::Device) -> Result<Command<T>, Error> {
        device.send(self)?;

        let response = device.read()?;
        Command::try_from(response)
    }
}

pub struct CommandBuilder<T: CommandDescriptor> {
    pub command: Command<T>,
}

impl<T: CommandDescriptor> CommandBuilder<T> {
    pub fn new(command: Command<T>) -> Self {
        Self { command }
    }

    pub fn build(self) -> Command<T> {
        self.command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libatk_derive::Command;

    #[derive(Command)]
    struct MacroCommand {}

    #[test]
    fn checksum_1() -> Result<(), Error> {
        let mut command = Command::<MacroCommand>::default();
        command.set_id(CommandId::SetEEPROM);
        command.set_status(0x00);
        command.set_eeprom_address(EEPROMAddress::Macro0);
        command.set_data_len(0x0a)?;
        command.set_data(
            &[0x08, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0xff],
            0x00,
        )?;

        assert_eq!(command.checksum, 0x22);
        Ok(())
    }

    #[test]
    fn checksum_2() -> Result<(), Error> {
        let mut command = Command::<MacroCommand>::default();
        command.set_id(CommandId::SetEEPROM);
        command.set_status(0x00);
        command.set_eeprom_address(EEPROMAddress::Macro0);
        command.set_eeprom_offset(0x0a);
        command.set_data_len(0x0a)?;
        command.set_data(
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            0x00,
        )?;

        assert_eq!(command.checksum, 0x39);
        Ok(())
    }

    #[test]
    fn checksum_3() -> Result<(), Error> {
        let mut command = Command::<MacroCommand>::default();
        command.set_id(CommandId::SetEEPROM);
        command.set_status(0x00);
        command.set_eeprom_address(EEPROMAddress::Macro0);
        command.set_eeprom_offset(0x14);
        command.set_data_len(0x0a)?;
        command.set_data(
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            0x00,
        )?;

        assert_eq!(command.checksum, 0x2f);
        Ok(())
    }

    #[test]
    fn checksum_4() -> Result<(), Error> {
        let mut command = Command::<MacroCommand>::default();
        command.set_id(CommandId::SetEEPROM);
        command.set_status(0x00);
        command.set_eeprom_address(EEPROMAddress::Macro3);
        command.set_eeprom_offset(0x00);
        command.set_data_len(0x0a)?;
        command.set_data(
            &[0x08, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0xff],
            0x00,
        )?;

        assert_eq!(
            command.checksum, 0x9e,
            "Expected checksum of 0x9E, but command is\n```\n{}\n```\ninstead",
            command
        );
        Ok(())
    }
}
