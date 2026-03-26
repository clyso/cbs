/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import { orderBy } from 'lodash-es';

export abstract class GeneralHelper {
  static orderBy = orderBy;

  static formatDate(date: string | Date): string {
    const dateToFormat = typeof date === 'string' ? new Date(date) : date;

    return dateToFormat.toLocaleDateString('en-US', {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
    });
  }

  static formatDateTime(date: string | Date, hasTimezone = false): string {
    const dateToFormat = typeof date === 'string' ? new Date(date) : date;

    return dateToFormat.toLocaleDateString('en-US', {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      timeZoneName: hasTimezone ? 'short' : undefined,
    });
  }
}
