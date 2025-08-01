import * as osd from '1fpga:osd';
import * as settings from '1fpga:settings';

import { getOrFail } from '@/services/settings/utils';
import { DbStorage } from '@/services/storage';
import { assert } from '@/utils';

export enum FontSize {
  Small = 'small',
  Medium = 'medium',
  Large = 'large',
}

export enum DateTimeFormat {
  Default = 'default',
  Short = 'short',
  TimeOnly = 'timeOnly',
  Hidden = 'hidden',
}

export enum DatetimeUpdate {
  Automatic = 'auto',
  Manual = 'manual',
  AutoWithTz = 'auto-tz',
}

export enum CatalogCheckFrequency {
  Manually = 'manually',
  EveryStartup = 'startup',
  Daily = 'daily',
  Weekly = 'weekly',
  Monthly = 'monthly',
}

const FONT_SIZE_KEY = 'fontSize';
const DATETIME_FORMAT_KEY = 'datetimeFormat';
const SHOW_FPS_KEY = 'showFps';
const INVERT_TOOLBAR_KEY = 'invertToolbar';
const TIMEZONE_KEY = 'timezone';
const DATETIME_UPDATE_KEY = 'datetimeUpdate';
const LAST_CATALOG_CHECK_DATE_KEY = 'lastCatalogCheck';
const CATALOG_CHECK_FREQUENCY_KEY = 'catalogCheckFrequency';
const VIDEO_MODE_KEY = 'videoMode';

export class GlobalSettings {
  public static async create() {
    return new GlobalSettings(await DbStorage.global());
  }

  public static async init() {
    const global = new GlobalSettings(await DbStorage.global());
    settings.setFontSize(await global.getFontSize());
    settings.setDatetimeFormat(await global.getDatetimeFormat());
    settings.setShowFps(await global.getShowFps());
    settings.setInvertToolbar(await global.getInvertToolbar());

    await global.updateDateTimeIfNecessary();
    await global.checkForUpdatesIfNecessary();
    return global;
  }

  private constructor(private readonly storage_: DbStorage) {}

  public async getFontSize(): Promise<FontSize> {
    return await getOrFail(this.storage_, FONT_SIZE_KEY, FontSize.Medium);
  }

  public async toggleFontSize() {
    const current = await this.getFontSize();
    const fontSizes = Object.values(FontSize);
    const currentIndex = fontSizes.indexOf(current);
    const next = fontSizes[(currentIndex + 1) % fontSizes.length];
    await this.setFontSize(next);
    return next;
  }

  public async setFontSize(value: FontSize): Promise<void> {
    assert.oneOfEnum(value, FontSize, 'Invalid font size');

    await this.storage_.set(FONT_SIZE_KEY, value);
    settings.setFontSize(value);
  }

  public async getDatetimeFormat(): Promise<DateTimeFormat> {
    return await getOrFail(this.storage_, DATETIME_FORMAT_KEY, DateTimeFormat.Default);
  }

  public async toggleDatetimeFormat() {
    const formats = Object.values(DateTimeFormat);
    const current = await this.getDatetimeFormat();
    const next = formats[(formats.indexOf(current) + 1) % formats.length];
    await this.setDatetimeFormat(next);
    return next;
  }

  public async setDatetimeFormat(value: DateTimeFormat): Promise<void> {
    assert.oneOfEnum(value, DateTimeFormat, 'Invalid datetime format');
    await this.storage_.set(DATETIME_FORMAT_KEY, value);
    settings.setDatetimeFormat(value);
  }

  public async getShowFps(): Promise<boolean> {
    return await getOrFail(this.storage_, SHOW_FPS_KEY, false);
  }

  public async setShowFps(value: boolean): Promise<void> {
    await this.storage_.set(SHOW_FPS_KEY, value);
    settings.setShowFps(value);
  }

  public async toggleShowFps() {
    const next = !(await this.getShowFps());
    await this.setShowFps(next);
    return next;
  }

  public async getInvertToolbar(): Promise<boolean> {
    return await getOrFail(this.storage_, INVERT_TOOLBAR_KEY, false);
  }

  public async setInvertToolbar(value: boolean): Promise<void> {
    await this.storage_.set(INVERT_TOOLBAR_KEY, value);
    settings.setInvertToolbar(value);
  }

  public async toggleInvertToolbar() {
    const next = !(await this.getInvertToolbar());
    await this.setInvertToolbar(next);
    return next;
  }

  public async getTimeZone(d?: string) {
    return await getOrFail(this.storage_, TIMEZONE_KEY, d);
  }

  public async setTimeZone(tz: string) {
    // This will throw if the timezone is invalid.
    settings.setTimeZone(tz);
    await this.storage_.set(TIMEZONE_KEY, tz);
  }

  public async setDateTimeUpdate(value: DatetimeUpdate) {
    await this.storage_.set(DATETIME_UPDATE_KEY, value);
  }

  public async getDateTimeUpdate() {
    return await getOrFail(this.storage_, DATETIME_UPDATE_KEY, DatetimeUpdate.Manual);
  }

  public async setVideoMode(value: string) {
    await this.storage_.set(VIDEO_MODE_KEY, value);
  }

  public async getVideoMode() {
    let mode = await this.storage_.get(VIDEO_MODE_KEY);
    if (typeof mode === 'string') {
      return mode;
    } else {
      return undefined;
    }
  }

  public async getCatalogCheckFrequency() {
    return await getOrFail(
      this.storage_,
      CATALOG_CHECK_FREQUENCY_KEY,
      CatalogCheckFrequency.Manually,
    );
  }

  public async setCatalogCheckFrequency(frequency: CatalogCheckFrequency) {
    return this.storage_.set(CATALOG_CHECK_FREQUENCY_KEY, frequency);
  }

  public async toggleCatalogCheckFrequency() {
    const current = await this.getCatalogCheckFrequency();
    const frequencies = Object.values(CatalogCheckFrequency);
    const next = frequencies[(frequencies.indexOf(current) + 1) % frequencies.length];
    await this.setCatalogCheckFrequency(next);
    return next;
  }

  public async updateDateTimeIfNecessary() {
    const update = await this.getDateTimeUpdate();
    if (update != DatetimeUpdate.Manual) {
      let tz = undefined;
      if (update === DatetimeUpdate.AutoWithTz) {
        tz = await this.getTimeZone('UTC');
      }
      settings.updateDateTime(tz, update === DatetimeUpdate.Automatic);
    }
  }

  private async shouldCheckForUpdates(days: number) {
    const lastCheck = new Date(await getOrFail(this.storage_, LAST_CATALOG_CHECK_DATE_KEY, 0));
    return days === undefined
      ? true
      : Date.now() - lastCheck.getTime() >= days * 24 * 60 * 60 * 1000;
  }

  public async checkForUpdatesIfNecessary() {
    const checkFrequency = await this.getCatalogCheckFrequency();
    console.debug('Check frequency:', checkFrequency);

    let should = false;
    switch (checkFrequency) {
      case CatalogCheckFrequency.Daily:
        should = await this.shouldCheckForUpdates(1);
        break;
      case CatalogCheckFrequency.Weekly:
        should = await this.shouldCheckForUpdates(7);
        break;
      case CatalogCheckFrequency.Monthly:
        should = await this.shouldCheckForUpdates(31);
        break;
      case CatalogCheckFrequency.EveryStartup:
        should = true;
        break;
      case CatalogCheckFrequency.Manually:
        // Don't even care about updating the last check date.
        return;
    }

    if (should) {
      // await db.catalog.checkForUpdates();
      await osd.alert('TODO: do update');
    }
    await this.storage_.set(LAST_CATALOG_CHECK_DATE_KEY, +Date.now());
  }
}
